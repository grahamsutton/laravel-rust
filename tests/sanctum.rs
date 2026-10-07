//! Laravel Sanctum through the `laravel` crate: discovered automatically,
//! authenticating API requests with personal access tokens.
#![cfg(feature = "sanctum")]

use laravel::prelude::*;
use laravel::sanctum::{CreatePersonalAccessTokensTable, HasApiTokens};
use laravel::testing::TestApp;

#[derive(Debug, Clone, Default, Model, Authenticatable)]
#[fillable(name, email, password)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub password: String,
}

struct CreateUsersTable;

#[async_trait]
impl Migration for CreateUsersTable {
    async fn up(&self) -> Result<()> {
        Schema::create("users", |table| {
            table.id();
            table.string("name");
            table.string("email");
            table.string("password");
        })
        .await
    }
}

async fn app() -> TestApp {
    let dir = tempfile::tempdir().unwrap().keep();
    let mut app = TestApp::new(
        Application::configure_detached(&dir)
            .with_routing(|routing| {
                routing.api(|| {
                    Route::get("/user", |request: Request| async move {
                        let user: User = request.user().unwrap();
                        Json(json!({"name": user.name}))
                    })
                    .middleware("auth:sanctum");
                    Route::get("/orders", || async { "Orders" }).middleware(["auth:sanctum", "abilities:orders:read"]);
                });
            })
            .with_migrations(laravel::database::migrations![
                "0001_01_01_000000_create_users_table" => CreateUsersTable,
                "2019_12_14_000001_create_personal_access_tokens_table" => CreatePersonalAccessTokensTable,
            ]),
    );
    app.refresh_database().await;
    app
}

async fn taylor() -> User {
    User::create(json!({"name": "Taylor", "email": "taylor@laravel.com", "password": "secret"})).await.unwrap()
}

#[tokio::test]
async fn api_requests_authenticate_with_tokens() {
    let mut app = app().await;
    let taylor = taylor().await;
    let token = taylor.create_token("cli", &["orders:read"]).await.unwrap().plain_text_token;

    app.get_json("/api/user").await.assert_unauthorized();

    app.with_header("Authorization", &format!("Bearer {token}"))
        .get_json("/api/user")
        .await
        .assert_ok()
        .assert_json(json!({"name": "Taylor"}));
}

#[tokio::test]
async fn token_abilities_are_enforced() {
    let mut app = app().await;
    let taylor = taylor().await;
    let reader = taylor.create_token("reader", &["orders:read"]).await.unwrap().plain_text_token;
    let limited = taylor.create_token("limited", &["profile"]).await.unwrap().plain_text_token;

    app.with_header("Authorization", &format!("Bearer {reader}")).get_json("/api/orders").await.assert_ok();
    app.with_header("Authorization", &format!("Bearer {limited}")).get_json("/api/orders").await.assert_forbidden();
}

#[tokio::test]
async fn tokens_issued_by_another_process_still_authenticate() {
    use sha2::Digest;

    let mut app = app().await;
    let taylor = taylor().await;
    // A token row written elsewhere: nothing in this process registered the
    // User model with Sanctum.
    let secret = "plain-text-secret-from-another-process";
    let id = laravel::database::DB::table("personal_access_tokens")
        .insert_get_id(json!({
            "tokenable_type": "User",
            "tokenable_id": taylor.id,
            "name": "deploy",
            "token": hex::encode(sha2::Sha256::digest(secret.as_bytes())),
            "abilities": "[\"*\"]",
        }))
        .await
        .unwrap();

    app.with_header("Authorization", &format!("Bearer {id}|{secret}"))
        .get_json("/api/user")
        .await
        .assert_ok()
        .assert_json(json!({"name": "Taylor"}));
}
