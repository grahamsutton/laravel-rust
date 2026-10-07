//! The framework, end to end: Eloquent models bound to routes, the
//! Eloquent user provider, sessions, and validation — through the `laravel`
//! crate exactly as an application uses it.

use laravel::prelude::*;
use laravel::testing::TestApp;

#[derive(Debug, Clone, Default, Model, Authenticatable)]
#[fillable(name, email, password)]
#[hidden(password, remember_token)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    #[hashed]
    pub password: String,
    pub remember_token: Option<String>,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
}

struct CreateUsersTable;

#[async_trait]
impl Migration for CreateUsersTable {
    async fn up(&self) -> Result<()> {
        Schema::create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.string("password");
            table.remember_token();
            table.timestamps();
        })
        .await
    }
}

fn routes() {
    Route::get("/users/{user}", |user: User| async move { Json(user) });
    Route::get("/users/{user:email}/name", |user: User| async move { user.name });

    Route::get("/login", || async { "Log in" }).name("login");
    Route::post("/login", login);
    Route::post("/register", register);

    Route::get("/dashboard", |request: Request| async move {
        let user: User = request.user().unwrap();
        format!("Welcome back, {}!", user.name)
    })
    .middleware("auth");
}

/// Handle an authentication attempt.
async fn login(request: Request) -> Result<Response> {
    let credentials = request
        .validate(rules! {
            "email" => "required|email",
            "password" => "required",
        })
        .await?;

    if Auth::attempt(&credentials, false).await? {
        request.session().regenerate(false).await?;

        return Ok(redirect_intended("/dashboard"));
    }

    Ok(back().with_errors(json!({"email": "The provided credentials do not match our records."})))
}

/// Handle an incoming registration request.
async fn register(request: Request) -> Result<Response> {
    let validated = request
        .validate(rules! {
            "name" => "required|string|max:255",
            "email" => "required|string|lowercase|email|max:255|unique:users,email",
            "password" => "required|confirmed|min:8",
        })
        .await?;

    let user = User::create(validated).await?;

    Auth::login(&user, false).await?;

    Ok(redirect("/dashboard"))
}

async fn app() -> TestApp {
    let dir = tempfile::tempdir().unwrap().keep();
    let mut app = TestApp::new(
        Application::configure_detached(&dir)
            .with_routing(|routing| {
                routing.web(routes);
            })
            .with_migrations(laravel::database::migrations![
                "0001_01_01_000000_create_users_table" => CreateUsersTable,
            ]),
    );
    app.refresh_database().await;
    app
}

async fn taylor() -> User {
    User::create(json!({
        "name": "Taylor Otwell",
        "email": "taylor@laravel.com",
        "password": "secret",
    }))
    .await
    .unwrap()
}

#[tokio::test]
async fn models_are_bound_to_routes() {
    let mut app = app().await;
    let taylor = taylor().await;

    app.get(&format!("/users/{}", taylor.id))
        .await
        .assert_ok()
        .assert_json_path("name", "Taylor Otwell")
        .assert_json_missing_path("password");

    app.get("/users/taylor@laravel.com/name").await.assert_ok().assert_see("Taylor Otwell");

    app.get_json("/users/999")
        .await
        .assert_not_found()
        .assert_json(json!({"message": "No query results for model [User] 999"}));
}

#[tokio::test]
async fn users_can_log_in_through_eloquent() {
    let mut app = app().await;
    let taylor = taylor().await;
    assert!(taylor.password.starts_with("$2y$"), "the password is hashed on save");

    app.get("/dashboard").await.assert_redirect("/login");
    app.assert_guest().await;

    app.post("/login", json!({"email": "taylor@laravel.com", "password": "wrong"}))
        .await
        .assert_session_has_errors(&["email"]);

    app.post("/login", json!({"email": "taylor@laravel.com", "password": "secret"}))
        .await
        .assert_redirect("/dashboard");
    app.assert_authenticated_as(&taylor).await;

    app.get("/dashboard").await.assert_ok().assert_see("Welcome back, Taylor Otwell!");
}

#[tokio::test]
async fn tests_can_act_as_a_model() {
    let mut app = app().await;
    let taylor = taylor().await;

    app.acting_as(&taylor).get("/dashboard").await.assert_see("Welcome back, Taylor Otwell!");
}

#[tokio::test]
async fn new_users_can_register() {
    let mut app = app().await;
    taylor().await;

    app.post(
        "/register",
        json!({
            "name": "Abigail Otwell",
            "email": "taylor@laravel.com",
            "password": "password",
            "password_confirmation": "password",
        }),
    )
    .await
    .assert_session_has_errors(&["email"]);

    app.post(
        "/register",
        json!({
            "name": "Abigail Otwell",
            "email": "abigail@laravel.com",
            "password": "password",
            "password_confirmation": "password",
        }),
    )
    .await
    .assert_redirect("/dashboard");

    app.assert_authenticated().await;
    assert_eq!(User::count().await.unwrap(), 2);
    let abigail = User::first_where("email", "abigail@laravel.com").await.unwrap().unwrap();
    assert!(Hash::check("password", &abigail.password));
}

#[tokio::test]
async fn models_can_be_asserted_in_the_database() {
    let app = app().await;
    let taylor = taylor().await;

    app.assert_model_exists(&taylor).await;
    taylor.clone().delete().await.unwrap();
    app.assert_model_missing(&taylor).await;
}
