use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_routing::Route;
use illuminate_support::json;
use illuminate_view::{Component, ComponentArgs, ComponentView, Factory, ViewData, data, view};

fn write(dir: &std::path::Path, path: &str, contents: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

struct Alert {
    kind: String,
}

impl Component for Alert {
    fn render(&self) -> ComponentView {
        ComponentView::inline(
            r#"<div {{ $attributes->merge(['class' => 'alert alert-'.$type]) }}>{{ $message }}</div>"#,
        )
    }

    fn data(&self) -> ViewData {
        data([("type", self.kind.as_str()), ("message", "Whoops & oh no!")])
    }
}

struct ProfileCard {
    name: String,
}

impl Component for ProfileCard {
    fn render(&self) -> ComponentView {
        ComponentView::view("components.profile-card")
    }

    fn data(&self) -> ViewData {
        data([("name", self.name.as_str())])
    }
}

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "resources/views/profile.blade.html",
        "<h1>{{ $name }}</h1>\n<p>{{ $bio }}</p>\n<ul>@foreach ($skills as $skill)<li>{{ $skill }}</li>@endforeach</ul>",
    );
    write(
        dir.path(),
        "resources/views/form.blade.html",
        "<input name=\"name\">\n@error('name')<span>{{ $message }}</span>@enderror\n@error('email', 'login')<span>{{ $message }}</span>@enderror",
    );
    write(dir.path(), "resources/views/empty.blade.html", "");
    write(
        dir.path(),
        "resources/views/components/profile-card.blade.html",
        "<div class=\"card\">{{ $name }}</div>",
    );

    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/profile", || async {
                view(
                    "profile",
                    json!({"name": "Taylor", "bio": "Creator of <Laravel>", "skills": ["PHP", "Rust"], "team": {"name": "Laravel"}}),
                )
            });
            Route::get("/text", || async { "Just text" });
        });
    });
    let app = TestApp::new(builder);
    Factory::resolve().composer("profile", |view| {
        view.set("followers", 42);
    });
    (app, dir)
}

// ----------------------------------------------------------------------
// Views on responses
// ----------------------------------------------------------------------

#[tokio::test]
async fn responses_know_which_view_they_rendered() {
    let (mut app, _dir) = test_app();
    Factory::resolve().share("app_name", "Laravel");

    let response = app.get("/profile").await;
    response
        .assert_ok()
        .assert_view_is("profile")
        .assert_view_has("name", None)
        .assert_view_has("name", json!("Taylor"))
        .assert_view_has("team.name", json!("Laravel"))
        .assert_view_has("followers", json!(42))
        .assert_view_has("app_name", json!("Laravel"))
        .assert_view_has_with("skills", |skills| {
            skills.as_array().is_some_and(|skills| skills.len() == 2)
        })
        .assert_view_has_all(json!({"name": "Taylor", "skills": ["PHP", "Rust"]}))
        .assert_view_has_all(json!(["name", "bio", "followers"]))
        .assert_view_missing("password")
        .assert_view_missing("team.owner");

    assert_eq!(response.view_data("name"), json!("Taylor"));
    assert_eq!(
        response.view_data(None)["bio"],
        json!("Creator of <Laravel>")
    );
    assert_eq!(response.view().unwrap().name, "profile");
    assert!(app.get("/text").await.view().is_none());
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that the response view [profile] is [welcome].")]
async fn assert_view_is_fails_for_other_views() {
    let (mut app, _dir) = test_app();
    app.get("/profile").await.assert_view_is("welcome");
}

#[tokio::test]
#[should_panic(expected = "The response is not a view.")]
async fn view_assertions_fail_for_responses_without_a_view() {
    let (mut app, _dir) = test_app();
    app.get("/text").await.assert_view_has("name", None);
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that the data contains the key [password].")]
async fn assert_view_has_fails_for_missing_keys() {
    let (mut app, _dir) = test_app();
    app.get("/profile").await.assert_view_has("password", None);
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that [name] matches the expected value.")]
async fn assert_view_has_fails_for_other_values() {
    let (mut app, _dir) = test_app();
    app.get("/profile")
        .await
        .assert_view_has("name", json!("Abigail"));
}

#[tokio::test]
#[should_panic(
    expected = "Failed asserting that the value at [name] fulfills the expectations defined by the closure."
)]
async fn assert_view_has_with_fails_when_the_closure_returns_false() {
    let (mut app, _dir) = test_app();
    app.get("/profile")
        .await
        .assert_view_has_with("name", |name| name == "Abigail");
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that the data does not contain the key [bio].")]
async fn assert_view_missing_fails_for_present_keys() {
    let (mut app, _dir) = test_app();
    app.get("/profile").await.assert_view_missing("bio");
}

// ----------------------------------------------------------------------
// Rendering views directly
// ----------------------------------------------------------------------

#[tokio::test]
async fn views_can_be_rendered_without_a_request() {
    let (app, _dir) = test_app();

    let view = app.view(
        "profile",
        json!({"name": "Taylor", "bio": "Creator of <Laravel>", "skills": ["PHP", "Rust"]}),
    );
    view.assert_see("Taylor")
        .assert_see("Creator of <Laravel>")
        .assert_see_html("<h1>Taylor</h1>")
        .assert_dont_see("Abigail")
        .assert_dont_see_html("<h2>")
        .assert_see_in_order(&["Taylor", "Creator", "PHP", "Rust"])
        .assert_see_html_in_order(&["<h1>", "<p>", "<li>PHP</li>"])
        .assert_see_text("Taylor Creator of <Laravel>")
        .assert_see_text_in_order(&["Taylor", "PHP", "Rust"])
        .assert_dont_see_text("Abigail")
        .assert_view_has("name", json!("Taylor"))
        .assert_view_has("followers", json!(42))
        .assert_view_has_with("skills", |skills| skills[0] == "PHP")
        .assert_view_has_all(json!(["name", "bio"]))
        .assert_view_missing("password");

    assert_eq!(view.view_data("name"), json!("Taylor"));
    assert_eq!(view.view().name(), "profile");
    assert!(
        view.to_string()
            .starts_with("<h1>Taylor</h1>\n<p>Creator of &lt;Laravel&gt;</p>")
    );
    assert_eq!(view.rendered(), view.to_string());

    app.view("empty", ()).assert_view_empty();
}

#[tokio::test]
async fn blade_templates_can_be_rendered() {
    let (app, _dir) = test_app();

    app.blade("Hello, {{ $name }}!", json!({"name": "<b>Taylor</b>"}))
        .assert_see("<b>Taylor</b>")
        .assert_see_html("Hello, &lt;b&gt;Taylor&lt;/b&gt;!")
        .assert_dont_see_html("<b>Taylor</b>");

    Factory::resolve()
        .blade()
        .component("alert", |args: &mut ComponentArgs| {
            Ok(Alert {
                kind: args.take_string("type").unwrap_or_else(|| "info".into()),
            })
        });
    app.blade(
        r#"<x-alert :type="$type" class="mb-4" />"#,
        json!({"type": "error"}),
    )
    .assert_see_html(r#"<div class="alert alert-error mb-4">"#)
    .assert_see("Whoops & oh no!");
}

#[tokio::test]
async fn components_can_be_rendered_on_their_own() {
    let (app, _dir) = test_app();

    let alert = app.component(Alert {
        kind: "warning".into(),
    });
    alert
        .assert_see_html(r#"<div class="alert alert-warning">"#)
        .assert_see("Whoops & oh no!")
        .assert_see_text("Whoops & oh no!")
        .assert_see_in_order(&["Whoops", "oh no!"])
        .assert_dont_see("Laravel")
        .assert_dont_see_text("alert-warning");
    assert_eq!(alert.component().kind, "warning");
    assert_eq!(
        alert.to_string(),
        r#"<div class="alert alert-warning">Whoops &amp; oh no!</div>"#
    );

    app.component(ProfileCard {
        name: "Abigail".into(),
    })
    .assert_see_html(r#"<div class="card">Abigail</div>"#);
}

#[tokio::test]
async fn validation_errors_can_be_shared_with_views() {
    let (mut app, _dir) = test_app();

    app.view("form", ())
        .assert_dont_see("Please provide a valid name.");

    app.with_view_errors(json!({"name": ["Please provide a valid name."]}))
        .view("form", ())
        .assert_see("Please provide a valid name.");

    app.with_view_errors_in(
        json!({"email": "These credentials do not match our records."}),
        "login",
    )
    .view("form", ())
    .assert_see("These credentials do not match our records.");
}

#[tokio::test]
#[should_panic(expected = "contains \"Abigail\".")]
async fn test_view_assert_see_fails_with_phpunits_message() {
    let (app, _dir) = test_app();
    app.blade("<p>Taylor</p>", ()).assert_see("Abigail");
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that '<p>Taylor</p>' does not contain \"Taylor\".")]
async fn test_view_assert_dont_see_fails_when_present() {
    let (app, _dir) = test_app();
    app.blade("<p>Taylor</p>", ()).assert_dont_see("Taylor");
}

#[tokio::test]
#[should_panic(expected = "contains \"Taylor\" in specified order.")]
async fn test_view_assert_see_in_order_fails_when_out_of_order() {
    let (app, _dir) = test_app();
    app.blade("<p>Taylor</p><p>Abigail</p>", ())
        .assert_see_in_order(&["Abigail", "Taylor"]);
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that '<p>Taylor</p>' does not contain \"Taylor\".")]
async fn test_view_assert_dont_see_text_fails_when_present() {
    let (app, _dir) = test_app();
    app.blade("<p>Taylor</p>", ())
        .assert_dont_see_text("Taylor");
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that '<p>Taylor</p>' is empty.")]
async fn test_view_assert_view_empty_fails_for_content() {
    let (app, _dir) = test_app();
    app.blade("<p>Taylor</p>", ()).assert_view_empty();
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that [name] matches the expected value.")]
async fn test_view_assert_view_has_fails_for_other_values() {
    let (app, _dir) = test_app();
    app.blade("{{ $name }}", json!({"name": "Taylor"}))
        .assert_view_has("name", json!("Abigail"));
}

#[tokio::test]
#[should_panic(expected = "View [missing] not found.")]
async fn rendering_a_missing_view_fails_the_test() {
    let (app, _dir) = test_app();
    app.view("missing", ());
}

#[tokio::test]
#[should_panic(expected = "does not contain \"alert-warning\".")]
async fn test_component_assert_dont_see_fails_when_present() {
    let (app, _dir) = test_app();
    app.component(Alert {
        kind: "warning".into(),
    })
    .assert_dont_see("alert-warning");
}
