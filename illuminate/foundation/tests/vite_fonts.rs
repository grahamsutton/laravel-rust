//! `@fonts` (Vite font optimization) and `@context`.

use illuminate_foundation::testing::TestApp;
use illuminate_foundation::{Application, Vite};
use illuminate_routing::Route;
use illuminate_support::json;
use illuminate_view::view;

fn write(dir: &std::path::Path, path: &str, contents: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "public/build/fonts-manifest.json",
        &json!({
            "version": 1,
            "families": {"sans": {"family": "Inter"}, "mono": {"family": "JetBrains Mono"}},
            "preloads": [
                {"alias": "sans", "file": "assets/inter-400.woff2", "type": "font/woff2", "crossorigin": "anonymous"},
                {"alias": "mono", "file": "assets/jetbrains-400.woff2", "type": "font/woff2", "crossorigin": "anonymous"},
            ],
            "style": {
                "inline": "@font-face { font-family: Inter; }\n@font-face { font-family: JetBrains Mono; }\n",
                "familyStyles": {
                    "sans": "@font-face { font-family: Inter; }",
                    "mono": "@font-face { font-family: JetBrains Mono; }",
                },
                "variables": {"sans": "--font-sans: Inter, sans-serif;", "mono": "--font-mono: JetBrains Mono, monospace;"},
            },
        })
        .to_string(),
    );
    write(dir.path(), "resources/views/all.blade.html", "@fonts");
    write(dir.path(), "resources/views/sans.blade.html", "@fonts('sans')");
    write(
        dir.path(),
        "resources/views/tenant.blade.html",
        "@context('tenant')<p>{{ $value }}</p>@endcontext @context('missing') missing @endcontext",
    );

    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/all", || async { view("all", ()) });
            Route::get("/sans", || async { view("sans", ()) });
            Route::get("/tenant", || async {
                illuminate_log::Context::add("tenant", "acme");
                view("tenant", ())
            });
        });
    });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn every_font_is_preloaded_and_styled() {
    let (mut app, _dir) = app();

    let html = app.get("/all").await.assert_ok().content();

    assert_eq!(
        html,
        "<link rel=\"preload\" as=\"font\" href=\"http://localhost/build/assets/inter-400.woff2\" type=\"font/woff2\" crossorigin=\"anonymous\" />\n\
<link rel=\"preload\" as=\"font\" href=\"http://localhost/build/assets/jetbrains-400.woff2\" type=\"font/woff2\" crossorigin=\"anonymous\" />\n\
<style>\n@font-face { font-family: Inter; }\n@font-face { font-family: JetBrains Mono; }\n</style>"
    );
}

#[tokio::test]
async fn fonts_can_be_filtered_by_alias() {
    let (mut app, _dir) = app();

    let html = app.get("/sans").await.assert_ok().content();

    assert!(html.contains("inter-400.woff2"));
    assert!(!html.contains("jetbrains"));
    assert!(html.ends_with("<style>\n@font-face { font-family: Inter; }\n\n:root {\n  --font-sans: Inter, sans-serif;\n}\n</style>"), "{html}");
}

#[tokio::test]
async fn unknown_aliases_and_missing_manifests_are_reported() {
    let (_app, dir) = app();

    let error = Vite::fonts(Some(&["serif"])).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Font alias [serif] is not defined in the font manifest. Available aliases: sans, mono."
    );

    std::fs::remove_file(dir.path().join("public/build/fonts-manifest.json")).unwrap();
    Vite::use_fonts_manifest_filename("missing.json");
    assert_eq!(Vite::fonts(None).unwrap().to_string(), "");
}

#[tokio::test]
async fn context_values_are_available_in_views() {
    let (mut app, _dir) = app();

    app.get("/tenant").await.assert_ok().assert_see_html("<p>acme</p>").assert_dont_see("missing");
}
