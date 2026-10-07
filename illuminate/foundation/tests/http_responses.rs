//! Streamed responses, downloads, JSONP, input flashing, content
//! negotiation and session blocking — through the whole application.

use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_http::{Request, Response, StreamedEvent, StreamedJson, response};
use illuminate_routing::Route;
use illuminate_support::{Error, json};

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/events", || async {
                response().event_stream(futures::stream::iter(vec![
                    StreamedEvent::from("started"),
                    StreamedEvent::new("progress", json!({"percent": 50})),
                ]))
            });
            Route::get("/users.json", || async {
                let users = futures::stream::iter((1..=3).map(|id| json!({"id": id})));
                response()
                    .stream_json(StreamedJson::new(json!({"total": 3})).with_stream("users", users))
            });
            Route::get("/export", || async {
                response().stream_download(
                    futures::stream::iter(vec![Ok::<_, Error>("id\n".into()), Ok("1\n".into())]),
                    Some("users.csv"),
                )
            });
            Route::get("/inline", || async {
                Response::stream_download_with_disposition(
                    futures::stream::iter(vec![Ok::<_, Error>("%PDF".into())]),
                    Some("invoice.pdf"),
                    "inline",
                )
            });
            Route::get("/report", || async {
                Response::download_content("data", "Q1 report.csv")
            });
            Route::get("/jsonp", |request: Request| async move {
                let callback = request.query("callback").as_str().map(str::to_string);
                Response::json(&json!({"name": "Taylor"})).with_callback(callback.as_deref())
            });
            Route::post("/login", || async {
                Response::redirect("/login").except_input(&["password"])
            });
            Route::post("/register", || async {
                Response::redirect("/register").only_input(&["email"])
            });
            Route::get("/negotiate", |request: Request| async move {
                match request.prefers(&["json", "markdown", "html"]).as_deref() {
                    Some("json") => Response::json(&json!({"format": "json"})),
                    Some("markdown") => Response::markdown("# Markdown"),
                    _ => Response::new("<h1>HTML</h1>"),
                }
            });
            Route::get("/away", |request: Request| async move {
                let to = request.query("to").as_str().unwrap_or("/").to_string();
                Response::redirect(to).enforce_same_origin("/home", true, true)
            });
            Route::post("/profile", |request: Request| async move {
                use illuminate_session::RequestSessionExt;
                request.session().put("saved", true);
                "Saved"
            })
            .block(10, 10);
        });
    });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn event_streams_reach_the_client() {
    let (mut app, _dir) = test_app();
    let response = app.get("/events").await;
    response
        .assert_ok()
        .assert_streamed()
        .assert_header("content-type", Some("text/event-stream"))
        .assert_header("cache-control", Some("no-cache"));
    response
        .assert_streamed_content(concat!(
            "event: update\ndata: started\n\n",
            "event: progress\ndata: {\"percent\":50}\n\n",
            "event: update\ndata: </stream>\n\n",
        ))
        .await;
}

#[tokio::test]
async fn json_is_streamed_item_by_item() {
    let (mut app, _dir) = test_app();
    let response = app.get("/users.json").await;
    response.assert_ok().assert_streamed();
    response
        .assert_streamed_json_content(
            json!({"total": 3, "users": [{"id": 1}, {"id": 2}, {"id": 3}]}),
        )
        .await;
}

#[tokio::test]
async fn downloads_are_offered_with_laravels_disposition() {
    let (mut app, _dir) = test_app();

    let response = app.get("/export").await;
    response
        .assert_ok()
        .assert_download(None)
        .assert_download(Some("users.csv"));
    response.assert_streamed_content("id\n1\n").await;

    app.get("/report")
        .await
        .assert_download(Some("Q1 report.csv"));

    let inline = app.get("/inline").await;
    let failure = AssertUnwindSafe(async {
        inline.assert_download(None);
    })
    .catch_unwind()
    .await
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<String>().unwrap(),
        "Response does not offer a file download.\nDisposition [inline] found in header, [attachment] expected."
    );

    let export = app.get("/export").await;
    let failure = AssertUnwindSafe(async {
        export.assert_download(Some("other.csv"));
    })
    .catch_unwind()
    .await
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<String>().unwrap(),
        "Expected file [other.csv] is not present in Content-Disposition header."
    );
}

#[tokio::test]
async fn jsonp_wraps_json_in_the_callback() {
    let (mut app, _dir) = test_app();
    app.get("/jsonp?callback=handleUser")
        .await
        .assert_ok()
        .assert_header("content-type", Some("text/javascript"))
        .assert_content(r#"/**/handleUser({"name":"Taylor"});"#);
    app.get("/jsonp")
        .await
        .assert_ok()
        .assert_json(json!({"name": "Taylor"}));
    app.get("/jsonp?callback=alert(document.cookie)")
        .await
        .assert_server_error();
}

#[tokio::test]
async fn redirects_flash_only_some_input() {
    let (mut app, _dir) = test_app();
    app.post(
        "/login",
        json!({"email": "taylor@laravel.com", "password": "secret"}),
    )
    .await
    .assert_redirect(Some("/login"))
    .assert_session_has_input("email", Some(json!("taylor@laravel.com")))
    .assert_session_missing_input("password");
    app.post(
        "/register",
        json!({"email": "taylor@laravel.com", "name": "Taylor"}),
    )
    .await
    .assert_session_has_input("email", None)
    .assert_session_missing_input("name");
}

#[tokio::test]
async fn content_is_negotiated() {
    let (mut app, _dir) = test_app();
    app.with_header("Accept", "text/markdown, text/html;q=0.9");
    app.get("/negotiate")
        .await
        .assert_header("content-type", Some("text/markdown"));
    app.with_header("Accept", "text/html, application/json;q=0.5");
    app.get("/negotiate").await.assert_see("HTML");
    app.with_header("Accept", "application/vnd.api+json");
    app.get("/negotiate")
        .await
        .assert_json(json!({"format": "json"}));
}

#[tokio::test]
async fn redirects_can_be_kept_on_the_same_origin() {
    let (mut app, _dir) = test_app();
    app.get("/away?to=https://evil.test/")
        .await
        .assert_redirect(Some("/home"));
    app.get("/away?to=http://localhost/dashboard")
        .await
        .assert_redirect(Some("http://localhost/dashboard"));
}

#[tokio::test]
async fn blocking_routes_run_with_the_session_lock() {
    let (mut app, _dir) = test_app();
    app.post("/profile", json!({}))
        .await
        .assert_ok()
        .assert_session_has("saved", Some(json!(true)));
    // The lock was released: the next request gets it again.
    app.post("/profile", json!({})).await.assert_ok();
}
