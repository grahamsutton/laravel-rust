use std::sync::Arc;

use illuminate_container::Container;
use illuminate_foundation::{Application, HttpKernel};
use illuminate_http::{Request, abort};
use illuminate_routing::Route;
use illuminate_support::json;

fn app() -> (Arc<Application>, illuminate_container::LocalInstanceGuard, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let app = Application::configure_detached(dir.path())
        .with_routing(|routing| {
            routing
                .web(|| {
                    Route::get("/", || async { "Hello, Laravel!" });
                    Route::get("/users/{id}", |illuminate_routing::Path(id): illuminate_routing::Path<u64>| async move {
                        json!({ "id": id })
                    });
                    Route::get("/forbidden", || async { abort::<&str>(403) });
                    Route::get("/panic", || async {
                        if true {
                            panic!("Whoops!");
                        }
                        "unreachable"
                    });
                })
                .api(|| {
                    Route::get("/status", || async { json!({"status": "ok"}) });
                })
                .health("/up");
        })
        .create();
    let guard = Container::set_local_instance(app.container().clone());
    app.bootstrap();
    app.config_repository().set("app.env", "testing");
    app.config_repository().set("session.driver", "array");
    app.config_repository().set("app.key", "base64:AckfSECXIvnK5r28GVIWUAxmbBSjTsmF+aDrMiCyByU=");
    (app, guard, dir)
}

#[tokio::test]
async fn it_handles_requests_through_the_full_stack() {
    let (app, _guard, _dir) = app();
    let kernel = HttpKernel::new(app.clone()).unwrap();

    let response = kernel.handle(Request::create("/", "GET")).await;
    if response.status_code() != 200 {
        eprintln!("{:?}", response.exception());
    }
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.content_string(), "Hello, Laravel!");

    let response = kernel.handle(Request::create("/users/42", "GET")).await;
    assert_eq!(response.json_body(), json!({"id": 42}));

    let response = kernel.handle(Request::create("/api/status", "GET")).await;
    assert_eq!(response.json_body(), json!({"status": "ok"}));

    let response = kernel.handle(Request::create("/up", "GET")).await;
    assert!(response.content_string().contains("Application up"));

    let response = kernel.handle(Request::create("/missing", "GET")).await;
    assert_eq!(response.status_code(), 404);

    let response = kernel.handle(Request::create("/forbidden", "GET")).await;
    assert_eq!(response.status_code(), 403);
    assert!(response.content_string().contains("Forbidden"));

    let response = kernel.handle(Request::create("/panic", "GET")).await;
    assert_eq!(response.status_code(), 500);
}
