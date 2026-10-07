use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_http::{Request, Response};
use illuminate_routing::Route;
use illuminate_session::RequestSessionExt;
use illuminate_support::json;

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/", || async { "<h1>Welcome to Laravel</h1>" });
            Route::get("/json", || async { json!({"data": [{"id": 1, "name": "Taylor"}, {"id": 2, "name": "Abigail"}]}) });
            Route::post("/counter", |request: Request| async move {
                let session = request.session();
                let count = session.increment("count");
                Response::redirect("/count").with("status", format!("Count is {count}"))
            });
            Route::get("/count", |request: Request| async move {
                format!("{}", request.session().get("count"))
            });
            Route::get("/name", |request: Request| async move {
                format!("Hello {}", request.session().get("name").as_str().unwrap_or("guest"))
            });
        });
    });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn it_makes_requests_and_asserts() {
    let (mut app, _dir) = test_app();

    app.get("/").await
        .assert_ok()
        .assert_see("Welcome to Laravel")
        .assert_see_text("Welcome")
        .assert_dont_see("Symfony");

    app.get_json("/json").await
        .assert_ok()
        .assert_json(json!({"data": [{"id": 1}]}))
        .assert_json_path("data.0.name", "Taylor")
        .assert_json_count(2, Some("data"))
        .assert_json_fragment(json!({"name": "Abigail"}))
        .assert_json_structure(json!({"data": {"*": ["id", "name"]}}));

    app.get("/missing").await.assert_not_found();
}

#[tokio::test]
async fn sessions_persist_across_requests() {
    let (mut app, _dir) = test_app();

    app.post("/counter", json!({})).await
        .assert_redirect("/count")
        .assert_session_has("status", Some(json!("Count is 1")));
    app.post("/counter", json!({})).await;
    app.get("/count").await.assert_see("2");

    app.with_session(json!({"name": "Taylor"}));
    app.get("/name").await.assert_see("Hello Taylor");
}

#[tokio::test]
async fn it_follows_redirects() {
    let (mut app, _dir) = test_app();
    app.following_redirects();
    app.post("/counter", json!({})).await.assert_ok().assert_see("1");
}
