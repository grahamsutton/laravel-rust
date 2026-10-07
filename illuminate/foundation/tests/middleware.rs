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
