//! The framework, end to end: Eloquent models bound to routes, the
//! Eloquent user provider, sessions, and validation — through the `laravel`
//! crate exactly as an application uses it.

use laravel::prelude::*;
use laravel::testing::TestApp;

#[derive(Debug, Clone, Default, Model, Authenticatable, Notifiable)]
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
        .await?;

        Schema::create("password_reset_tokens", |table| {
            table.string("email").primary();
            table.string("token");
            table.timestamp("created_at").nullable();
        })
        .await
    }
}

pub struct UserResource(pub User);

impl JsonResource for UserResource {
    type Model = User;

    fn from_model(user: User) -> Self {
        Self(user)
    }

    fn model(&self) -> &User {
        &self.0
    }

    fn to_array(&self, request: &Request) -> Value {
        json!({
            "id": self.0.id,
            "name": self.0.name,
            "email": self.when(request.boolean("with_email"), || self.0.email.clone()),
        })
    }
}

fn routes() {
    Route::get("/api/users/{user}", |user: User| async move { UserResource::make(user) });
    Route::get("/api/users", || async {
        Ok::<_, Error>(UserResource::collection(User::query().order_by("id", "asc").paginate(2).await?))
    });
    Route::get("/users/{user}", |user: User| async move { Json(user) });
    Route::get("/users/{user:email}/name", |user: User| async move { user.name });

    Route::get("/login", || async { "Log in" }).name("login");
    Route::get("/reset-password/{token}", || async { "Reset your password" }).name("password.reset");
    Route::get("/email/verify/{id}/{hash}", |request: Request| async move {
        if request.has_valid_signature() { "Verified" } else { "Invalid" }
    })
    .name("verification.verify");
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

#[tokio::test]
async fn models_are_transformed_by_api_resources() {
    let mut app = app().await;
    let taylor = taylor().await;
    for name in ["Abigail", "James"] {
        User::create(json!({"name": name, "email": format!("{}@laravel.com", name.to_lowercase()), "password": "secret"}))
            .await
            .unwrap();
    }

    app.get_json(&format!("/api/users/{}", taylor.id))
        .await
        .assert_ok()
        .assert_exact_json(json!({"data": {"id": taylor.id, "name": "Taylor Otwell"}}));

    app.get_json(&format!("/api/users/{}?with_email=1", taylor.id))
        .await
        .assert_json_path("data.email", "taylor@laravel.com");

    app.get_json("/api/users?page=2")
        .await
        .assert_ok()
        .assert_json_count(1, Some("data"))
        .assert_json_path("data.0.name", "James")
        .assert_json_path("meta.current_page", 2)
        .assert_json_path("meta.total", 3)
        .assert_json_path("links.prev", "http://localhost/api/users?page=1");
}

#[tokio::test]
async fn passwords_can_be_reset() {
    use laravel::facades::Password;

    let app = app().await;
    taylor().await;
    let sent = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));

    let link = sent.clone();
    let status = Password::send_reset_link(&json!({"email": "taylor@laravel.com"}), |user: User, token: String| async move {
        assert_eq!(user.email, "taylor@laravel.com");
        *link.lock().unwrap() = Some(token);
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(status.as_str(), "passwords.sent");
    let token = sent.lock().unwrap().clone().unwrap();
    app.assert_database_count("password_reset_tokens", 1).await;

    let throttled = Password::send_reset_link(&json!({"email": "taylor@laravel.com"}), |_: User, _: String| async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(throttled.as_str(), "passwords.throttled");

    let reset = |token: String| async move {
        Password::reset(
            &json!({"email": "taylor@laravel.com", "password": "new-secret", "token": token}),
            |mut user: User, password: String| async move {
                user.password = password;
                user.save().await?;
                Ok(())
            },
        )
        .await
        .unwrap()
    };
    assert_eq!(reset("wrong-token".into()).await.as_str(), "passwords.token");
    assert_eq!(reset(token.clone()).await.as_str(), "passwords.reset");
    assert_eq!(reset(token).await.as_str(), "passwords.token", "tokens can only be used once");

    assert!(Auth::validate(&json!({"email": "taylor@laravel.com", "password": "new-secret"})).await.unwrap());
}

#[tokio::test]
async fn reset_tokens_can_live_in_the_cache() {
    use laravel::facades::Password;

    let app = app().await;
    app.app().override_config("auth.passwords.users.driver", "cache");
    let taylor = taylor().await;

    let token = Password::create_token(&AuthUser::from(&taylor)).await.unwrap();

    assert!(Password::token_exists(&AuthUser::from(&taylor), &token).await.unwrap());
    app.assert_database_empty("password_reset_tokens").await;
}

fn sent_mail() -> Vec<laravel::mail::SentMessage> {
    let mailer = laravel::container::app::<laravel::mail::MailManager>().mailer(Some("array")).unwrap();
    laravel::mail::downcast_transport::<laravel::mail::ArrayTransport>(&mailer.transport())
        .unwrap()
        .messages()
}

#[tokio::test]
async fn reset_links_are_emailed() {
    use laravel::foundation::auth::notifications::send_password_reset_link;

    let _app = app().await;
    taylor().await;

    let status = send_password_reset_link::<User>(&json!({"email": "taylor@laravel.com"})).await.unwrap();
    assert_eq!(status.as_str(), "passwords.sent");

    let sent = sent_mail();
    assert_eq!(sent.len(), 1);
    let message = &sent[0].message;
    assert_eq!(message.subject.as_deref(), Some("Reset Password Notification"));
    let html = message.html.as_deref().unwrap();
    assert!(html.contains("http://localhost/reset-password/"));
    assert!(html.contains("email=taylor%40laravel.com"));
    assert!(html.contains("This password reset link will expire in 60 minutes."));
}

#[tokio::test]
async fn verification_links_are_signed() {
    use laravel::foundation::auth::notifications::{VerifyEmail, send_email_verification_notification};

    let mut app = app().await;
    let taylor = taylor().await;

    send_email_verification_notification(&taylor).await.unwrap();
    let message = &sent_mail()[0].message;
    assert_eq!(message.subject.as_deref(), Some("Verify Email Address"));

    let url = VerifyEmail.verification_url(&taylor);
    assert!(url.contains("signature="));
    let path = url.trim_start_matches("http://localhost");
    app.get(path).await.assert_see("Verified");
    app.get(&path.replace("signature=", "signature=tampered")).await.assert_see("Invalid");
}
