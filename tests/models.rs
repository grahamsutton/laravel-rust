//! Eloquent's application-level features: model:show, model:prune, model
//! events through the event dispatcher, and dirty tracking.

use std::sync::atomic::{AtomicUsize, Ordering};

use laravel::prelude::*;
use laravel::testing::TestApp;

#[derive(Debug, Clone, Default, Model)]
#[fillable(airline, destination)]
pub struct Flight {
    pub id: u64,
    pub airline: String,
    pub destination: String,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
    pub original: Original,
}

impl Prunable for Flight {
    fn prunable() -> Builder<Self> {
        Flight::query().where_op("created_at", "<=", Carbon::now().sub_months(1))
    }
}

struct CreateFlightsTable;

#[async_trait]
impl Migration for CreateFlightsTable {
    async fn up(&self) -> Result<()> {
        Schema::create("flights", |table| {
            table.id();
            table.string("airline");
            table.string("destination");
            table.timestamps();
        })
        .await
    }
}

async fn app() -> TestApp {
    let dir = tempfile::tempdir().unwrap().keep();
    let mut app = TestApp::new(Application::configure_detached(&dir).with_migrations(laravel::database::migrations![
        "2024_01_01_000000_create_flights_table" => CreateFlightsTable,
    ]));
    app.refresh_database().await;
    app
}

async fn flight(destination: &str) -> Flight {
    Flight::create(json!({"airline": "Laravel Air", "destination": destination})).await.unwrap()
}

#[tokio::test]
async fn models_can_be_inspected() {
    let app = app().await;

    app.artisan("model:show Flight")
        .expects_output_to_contain("Flight")
        .expects_output_to_contain("flights")
        .expects_output_to_contain("Attributes")
        .expects_output_to_contain("destination")
        .assert_successful()
        .await;

    app.artisan("model:show Airport")
        .expects_output_to_contain("Model [Airport] not found.")
        .assert_failed()
        .await;
}

#[tokio::test]
async fn stale_models_are_pruned() {
    let mut app = app().await;
    app.travel(-2).months();
    flight("Lisbon").await;
    flight("Porto").await;
    app.travel_back();
    flight("Madeira").await;

    app.artisan("model:prune --pretend")
        .expects_output_to_contain("2 [Flight] records will be pruned.")
        .assert_successful()
        .await;
    assert_eq!(Flight::count().await.unwrap(), 3);

    app.artisan("model:prune --model=Flight")
        .expects_output_to_contain("Pruning [Flight] records.")
        .assert_successful()
        .await;
    assert_eq!(Flight::count().await.unwrap(), 1);

    app.artisan("model:prune")
        .expects_output_to_contain("No prunable [Flight] records found.")
        .assert_successful()
        .await;
}

static CREATED: AtomicUsize = AtomicUsize::new(0);

#[tokio::test]
async fn model_events_reach_event_listeners() {
    let _app = app().await;
    Event::listen_named("eloquent.created: Flight", |_: &str, payload: &Value| {
        assert_eq!(payload["destination"], "Lisbon");
        CREATED.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });

    flight("Lisbon").await;

    assert_eq!(CREATED.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn only_changed_attributes_are_saved() {
    let _app = app().await;
    let mut flight = flight("Lisbon").await;
    assert!(flight.is_clean());

    flight.destination = "Porto".into();
    assert!(flight.is_dirty_any("destination"));
    assert!(flight.is_clean_all("airline"));
    assert_eq!(flight.get_original_attribute("destination"), json!("Lisbon"));

    flight.save().await.unwrap();
    assert!(flight.is_clean());
    assert!(flight.was_changed_any("destination"));
    assert_eq!(Flight::find(flight.id).await.unwrap().unwrap().destination, "Porto");
}

#[tokio::test]
async fn models_are_cursor_paginated_from_the_request() {
    let dir = tempfile::tempdir().unwrap().keep();
    let mut app = TestApp::new(
        Application::configure_detached(&dir)
            .with_routing(|routing| {
                routing.web(|| {
                    Route::get("/flights", || async {
                        let page = Flight::query().order_by("id", "asc").cursor_paginate(2, None).await?;
                        Ok::<_, Error>(Json(page))
                    });
                });
            })
            .with_migrations(laravel::database::migrations![
                "2024_01_01_000000_create_flights_table" => CreateFlightsTable,
            ]),
    );
    app.refresh_database().await;
    for destination in ["Lisbon", "Oslo", "Tokyo", "Lima", "Cairo"] {
        flight(destination).await;
    }

    let first = app.get("/flights").await.assert_ok().json();
    assert_eq!(first["data"][0]["destination"], "Lisbon");
    assert_eq!(first["data"].as_array().unwrap().len(), 2);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();

    let second = app.get(&format!("/flights?cursor={cursor}")).await.assert_ok().json();
    assert_eq!(second["data"][0]["destination"], "Tokyo");
    assert_eq!(second["data"][1]["destination"], "Lima");
    assert!(second["prev_cursor"].is_string());
}
