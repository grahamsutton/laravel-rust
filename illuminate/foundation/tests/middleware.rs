use illuminate_foundation::Application;
use illuminate_foundation::http::middleware::{AddLinkHeadersForPreloadedAssets, TrustHosts};
use illuminate_foundation::testing::TestApp;
use illuminate_routing::{Route, RouteMiddleware};

fn app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("public/build")).unwrap();
    std::fs::write(
        dir.path().join("public/build/manifest.json"),
        r#"{"resources/js/app.js": {"file": "assets/app-4ed993c7.js", "src": "resources/js/app.js", "isEntry": true, "css": ["assets/app-c2ab3b2b.css"]}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("resources/views")).unwrap();
    std::fs::write(dir.path().join("resources/views/welcome.blade.html"), "@vite(['resources/js/app.js'])").unwrap();

    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/", || async { illuminate_view::view("welcome", ()) })
                .middleware(RouteMiddleware::of(AddLinkHeadersForPreloadedAssets::default()));
            Route::get("/trusted", || async { "Trusted" })
                .middleware(RouteMiddleware::of(TrustHosts::at(&["^laravel\\.test$"], false).always()));
        });
    });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn preloaded_assets_are_sent_as_link_headers() {
    let (mut app, _dir) = app();

    let response = app.get("/").await;
    response.assert_ok();

    let link = response.header("Link").unwrap();
    assert!(link.contains(r#"/build/assets/app-c2ab3b2b.css>; rel="preload"; as="style""#), "{link}");
    assert!(link.contains(r#"/build/assets/app-4ed993c7.js>; rel="modulepreload"; as="script""#), "{link}");

    // Each request preloads its own assets.
    let second = app.get("/").await;
    assert_eq!(second.header("Link").unwrap(), link);
}

#[tokio::test]
async fn only_trusted_hosts_are_answered() {
    let (mut app, _dir) = app();

    app.with_header("Host", "laravel.test").get("/trusted").await.assert_ok().assert_see("Trusted");
    app.with_header("Host", "evil.example").get("/trusted").await.assert_status(400);
}

#[tokio::test]
async fn each_request_gets_its_own_csp_nonce() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("public/build")).unwrap();
    std::fs::write(
        dir.path().join("public/build/manifest.json"),
        r#"{"resources/js/app.js": {"file": "assets/app.js", "src": "resources/js/app.js", "isEntry": true}}"#,
    )
    .unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/", || async {
                let nonce = illuminate_foundation::Vite::use_csp_nonce(None);
                // Let other requests run before rendering.
                tokio::task::yield_now().await;
                let tags = illuminate_foundation::Vite::render(&["resources/js/app.js"]).unwrap();
                assert!(tags.to_string().contains(&format!("nonce=\"{nonce}\"")));
                nonce
            });
        });
    });
    let app = TestApp::new(builder);

    let requests = (0..8).map(|_| {
        let request = illuminate_http::Request::create("/", "GET");
        app.app().handle_request(request)
    });
    let nonces: std::collections::HashSet<String> = futures::future::join_all(requests)
        .await
        .into_iter()
        .map(|response| {
            assert_eq!(response.status().as_u16(), 200);
            response.content_string()
        })
        .collect();
    assert_eq!(nonces.len(), 8);
}

/// What `make:middleware` generates: a struct registered with
/// `register_middleware!`, appended to the global stack by type.
struct AddServerTiming;

illuminate_routing::register_middleware!(AddServerTiming);

#[illuminate_http::async_trait]
impl illuminate_http::Middleware for AddServerTiming {
    async fn handle(
        &self,
        request: illuminate_http::Request,
        next: illuminate_http::Next,
    ) -> illuminate_support::Result<illuminate_http::Response> {
        let mut response = next.run(request).await;
        response.set_header("Server-Timing", format!("app;dur={:.2}", 1.5));
        Ok(response)
    }
}

#[tokio::test]
async fn registered_middleware_types_can_be_appended_by_type() {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path())
        .with_routing(|routing| {
            routing.web(|| {
                Route::get("/", || async { "Hello" });
                Route::get("/route", || async { "Route" }).middleware(AddServerTiming);
            });
        })
        .with_middleware(|middleware| {
            middleware.append(AddServerTiming);
        });
    let mut app = TestApp::new(builder);

    app.get("/").await.assert_ok().assert_header("Server-Timing", Some("app;dur=1.50"));
    app.get("/route").await.assert_ok().assert_header("Server-Timing", Some("app;dur=1.50"));
}
