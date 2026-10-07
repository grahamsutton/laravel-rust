use illuminate_auth::GenericUser;
use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_http::Request;
use illuminate_routing::Route;
use illuminate_support::json;
use illuminate_validation::{Validator, rules};
use illuminate_view::view;

fn write(dir: &std::path::Path, path: &str, contents: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "resources/views/welcome.blade.html",
        r#"<h1>{{ __('Welcome, :name', ['name' => $name]) }}</h1>
<a href="{{ route('users.show', ['user' => 42]) }}">Profile</a>
@auth
    <p>Signed in as {{ auth()->user()['name'] }}</p>
@else
    <a href="{{ route('login') }}">Log in</a>
@endauth
"#,
    );
    write(
        dir.path(),
        "resources/views/form.blade.html",
        r#"<form method="POST" action="/posts">
    @csrf
    <input name="title" value="{{ old('title') }}">
    @error('title')
        <span class="error">{{ $message }}</span>
    @enderror
</form>
"#,
    );
    write(dir.path(), "resources/views/about.blade.html", "About {{ $company }}");
    write(
        dir.path(),
        "resources/views/errors/404.blade.html",
        "Lost? {{ $exception['message'] }} ({{ $code }})",
    );
    write(
        dir.path(),
        "resources/views/dashboard.blade.html",
        "Dashboard for {{ auth()->user()['name'] }}",
    );

    let builder = Application::configure_detached(dir.path())
        .with_routing(|routing| {
            routing.web(|| {
                Route::get("/", || async { view("welcome", json!({"name": "Taylor"})) });
                Route::get("/users/{user}", || async { "User" }).name("users.show");
                Route::get("/login", || async { "Login" }).name("login");
                Route::get("/form", || async { view("form", ()) });
                Route::post("/posts", |request: Request| async move {
                    Validator::make(request.all(), rules! { "title" => "required|min:3" })
                        .validate()
                        .await?;
                    Ok::<_, illuminate_support::Error>("Created")
                });
                Route::view("/about", "about", json!({"company": "Laravel"}));
                Route::get("/dashboard", || async { view("dashboard", ()) }).middleware("auth");
            });
        })
        .with_middleware(|middleware| {
            middleware.redirect_users_to("/dashboard");
        });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn views_can_use_framework_helpers() {
    let (mut app, _dir) = test_app();

    app.get("/")
        .await
        .assert_ok()
        .assert_see("Welcome, Taylor")
        .assert_see(r#"href="http://localhost/users/42""#)
        .assert_see(r#"href="http://localhost/login""#);

    app.get("/about").await.assert_ok().assert_see_text("About Laravel");
}

#[tokio::test]
async fn validation_errors_are_shared_with_views() {
    let (mut app, _dir) = test_app();

    app.get("/form").await.assert_ok().assert_see(r#"name="_token""#);

    app.from("/form")
        .post("/posts", json!({"title": "Hi"}))
        .await
        .assert_redirect("http://localhost/form")
        .assert_session_has_errors(&["title"]);

    app.get("/form")
        .await
        .assert_see("The title field must be at least 3 characters.")
        .assert_see(r#"value="Hi""#);
}

#[tokio::test]
async fn custom_error_pages_are_rendered() {
    let (mut app, _dir) = test_app();

    app.get("/missing").await.assert_not_found().assert_see("Lost?").assert_see("(404)");
}

#[tokio::test]
async fn guests_are_redirected_to_the_login_route() {
    let (mut app, _dir) = test_app();
    app.app().override_config(
        "auth.providers.users",
        json!({"driver": "array", "users": [{"id": 1, "name": "Taylor"}]}),
    );

    app.get("/dashboard").await.assert_redirect("http://localhost/login");
    app.get_json("/dashboard")
        .await
        .assert_unauthorized()
        .assert_json(json!({"message": "Unauthenticated."}));

    app.acting_as(&GenericUser::new(json!({"id": 1, "name": "Taylor"})));

    app.get("/dashboard").await.assert_ok().assert_see("Dashboard for Taylor");
    app.get("/").await.assert_see("Signed in as Taylor");
}

#[tokio::test]
async fn components_can_be_generated() {
    let (app, dir) = test_app();

    app.artisan("make:component AlertBanner")
        .expects_output_to_contain("Component [app/view/components/alert_banner.rs] created successfully.")
        .expects_output_to_contain("View [resources/views/components/alert-banner.blade.html] created successfully.")
        .assert_successful()
        .await;
    app.artisan("make:component Forms/Input --view")
        .expects_output_to_contain("View [resources/views/components/forms/input.blade.html] created successfully.")
        .assert_successful()
        .await;

    let class = std::fs::read_to_string(dir.path().join("app/view/components/alert_banner.rs")).unwrap();
    assert!(class.contains("impl Component for AlertBanner"));
    assert!(class.contains(r#"ComponentView::view("components.alert-banner")"#));
}

#[tokio::test]
async fn views_can_be_cached_and_cleared() {
    let (app, dir) = test_app();
    let views = dir.path().join("resources/views");
    std::fs::create_dir_all(views.join("admin")).unwrap();
    std::fs::write(views.join("admin/dashboard.blade.html"), "@if($ok) Hello @endif").unwrap();

    app.artisan("view:cache")
        .expects_output_to_contain("Blade templates cached successfully.")
        .assert_successful()
        .await;
    app.artisan("optimize")
        .expects_output_to_contain("Caching framework bootstrap, configuration, and metadata.")
        .expects_output_to_contain("views")
        .assert_successful()
        .await;

    std::fs::write(views.join("broken.blade.html"), "@foreach($users as $user) {{ $user }}").unwrap();
    app.artisan("view:cache")
        .expects_output_to_contain("broken.blade.html")
        .assert_failed()
        .await;

    app.artisan("view:clear")
        .expects_output_to_contain("Compiled views cleared successfully.")
        .assert_successful()
        .await;
    app.artisan("optimize:clear")
        .expects_output_to_contain("Clearing cached bootstrap files.")
        .assert_successful()
        .await;
}

#[tokio::test]
async fn custom_pagination_views_are_rendered_with_blade() {
    use illuminate_pagination::{LengthAwarePaginator, PaginatorOptions};

    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "resources/views/pagination/links.blade.html",
        r#"<nav>Page {{ $paginator['current_page'] }} of {{ $paginator['total'] }}@if ($paginator['has_more_pages']) <a href="{{ $paginator['next_page_url'] }}">Next</a>@endif</nav>"#,
    );
    let _app = TestApp::new(Application::configure_detached(dir.path()));

    let paginator = LengthAwarePaginator::new(vec![1, 2], 6, 2, 2, PaginatorOptions::path("/users"));

    assert_eq!(
        paginator.links_with("pagination.links").to_string().trim(),
        r#"<nav>Page 2 of 6 <a href="/users?page=3">Next</a></nav>"#
    );
}
