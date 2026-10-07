//! Issuing, finding, inspecting and revoking personal access tokens.

use illuminate_database::eloquent::*;
use illuminate_support::Str;
use laravel_sanctum::{HasAbilities, HasApiTokens, NewAccessToken, PersonalAccessToken};

use crate::support::{Admin, User, abigail, app, app_with, taylor, travel_to};

fn sha256(value: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[tokio::test]
async fn tokens_are_issued_as_id_pipe_secret_and_stored_hashed() {
    let _app = app().await;
    let user = taylor().await;

    let token = user.create_token("token-name", &["*"]).await.unwrap();

    let (id, secret) = token.plain_text_token.split_once('|').unwrap();
    assert_eq!(id, token.access_token.id.to_string());
    assert_eq!(
        secret.len(),
        48,
        "40 random characters and an 8 character checksum"
    );
    assert!(secret.chars().all(|c| c.is_ascii_alphanumeric()));

    let stored = PersonalAccessToken::find_or_fail(token.access_token.id)
        .await
        .unwrap();
    assert_eq!(stored.token, sha256(secret));
    assert_ne!(stored.token, secret, "plain-text tokens are never stored");
    assert_eq!(stored.name, "token-name");
    assert_eq!(stored.abilities, ["*"]);
    assert_eq!(stored.tokenable_type, "User");
    assert_eq!(stored.tokenable_id, json!(user.id));
    assert!(stored.last_used_at.is_none());
    assert!(stored.expires_at.is_none());
    assert!(stored.created_at.is_some());
}

#[tokio::test]
async fn tokens_end_with_a_checksum_of_their_entropy() {
    let _app = app().await;
    let user = taylor().await;

    Str::create_random_strings_using_sequence(vec!["a".repeat(40)]);
    let token = user.create_token("checksum", &["*"]).await.unwrap();
    Str::create_random_strings_normally();

    let secret = token.plain_text_token.split_once('|').unwrap().1;
    assert_eq!(secret, format!("{}c95b8a25", "a".repeat(40)));
}

#[tokio::test]
async fn tokens_are_prefixed_with_the_configured_token_prefix() {
    let _app = app_with(json!({"sanctum": {"token_prefix": "laravel_"}})).await;
    let user = taylor().await;

    Str::create_random_strings_using_sequence(vec!["b".repeat(40)]);
    let token = user.create_token("prefixed", &["*"]).await.unwrap();
    Str::create_random_strings_normally();

    let secret = token.plain_text_token.split_once('|').unwrap().1;
    assert_eq!(secret, format!("laravel_{}731b405a", "b".repeat(40)));
    assert!(
        PersonalAccessToken::find_token(&token.plain_text_token)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn tokens_may_be_granted_abilities() {
    let _app = app().await;
    let user = taylor().await;

    let token = user
        .create_token("deploy", &["server:update", "server:reboot"])
        .await
        .unwrap();
    let stored = PersonalAccessToken::find_or_fail(token.access_token.id)
        .await
        .unwrap();

    assert_eq!(stored.abilities, ["server:update", "server:reboot"]);
    assert!(stored.can("server:update"));
    assert!(stored.can("server:reboot"));
    assert!(stored.cant("server:delete"));
    assert!(HasAbilities::can(&stored, "server:update"));
    assert!(HasAbilities::cant(&stored, "server:delete"));

    let everything = user.create_token("admin", &["*"]).await.unwrap();
    assert!(everything.access_token.can("anything:at-all"));

    let nothing = user.create_token("read-only", &[]).await.unwrap();
    let stored = PersonalAccessToken::find_or_fail(nothing.access_token.id)
        .await
        .unwrap();
    assert!(stored.abilities.is_empty());
    assert!(stored.cant("server:update"));
}

#[tokio::test]
async fn tokens_may_expire_at_a_given_time() {
    let _app = app().await;
    let now = travel_to("2025-01-01 12:00:00");
    let user = taylor().await;

    let token = user
        .create_token_with_expiry("temporary", &["*"], now.add_week())
        .await
        .unwrap();

    let stored = PersonalAccessToken::find_or_fail(token.access_token.id)
        .await
        .unwrap();
    assert_eq!(
        stored.expires_at.unwrap().to_date_time_string(),
        "2025-01-08 12:00:00"
    );
    assert!(!stored.is_expired());

    travel_to("2025-01-08 12:00:01");
    assert!(stored.is_expired());
}

#[tokio::test]
async fn the_new_access_token_serializes_like_laravel() {
    let _app = app().await;
    let user = taylor().await;
    let token: NewAccessToken = user.create_token("token-name", &["*"]).await.unwrap();

    let array = token.to_array();
    assert_eq!(array["plainTextToken"], json!(token.plain_text_token));
    assert_eq!(array["accessToken"]["name"], json!("token-name"));
    assert_eq!(array["accessToken"]["abilities"], json!(["*"]));
    assert!(
        array["accessToken"].get("token").is_none(),
        "the hashed token is hidden"
    );
    assert_eq!(token.to_json(), array.to_string());
}

#[tokio::test]
async fn tokens_are_found_by_id_and_secret() {
    let _app = app().await;
    let user = taylor().await;
    let token = user.create_token("token-name", &["*"]).await.unwrap();
    let (id, secret) = token.plain_text_token.split_once('|').unwrap();

    let found = PersonalAccessToken::find_token(&token.plain_text_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, token.access_token.id);

    // Bare tokens are found by their hash...
    let found = PersonalAccessToken::find_token(secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, token.access_token.id);

    // ...while a wrong secret, ID, or token finds nothing.
    let wrong_secret = format!("{id}|{}", "x".repeat(48));
    assert!(
        PersonalAccessToken::find_token(&wrong_secret)
            .await
            .unwrap()
            .is_none()
    );
    let wrong_id = format!("999|{secret}");
    assert!(
        PersonalAccessToken::find_token(&wrong_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        PersonalAccessToken::find_token("nope")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        PersonalAccessToken::find_token("abc|def")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn the_tokens_relationship_is_scoped_to_its_owner() {
    let _app = app().await;
    let taylor = taylor().await;
    let abigail = abigail().await;
    let admin = Admin::create(json!({"name": "Root"})).await.unwrap();
    assert_eq!(admin.id, taylor.id, "same key, different tokenable type");

    taylor.create_token("laptop", &["*"]).await.unwrap();
    taylor.create_token("phone", &["*"]).await.unwrap();
    abigail.create_token("laptop", &["*"]).await.unwrap();
    admin.create_token("console", &["*"]).await.unwrap();

    let names: Vec<String> = taylor
        .tokens()
        .get()
        .await
        .unwrap()
        .into_iter()
        .map(|token| token.name)
        .collect();
    assert_eq!(names, ["laptop", "phone"]);
    assert_eq!(abigail.tokens().count().await.unwrap(), 1);
    assert_eq!(admin.tokens().count().await.unwrap(), 1);
    assert_eq!(
        admin
            .tokens()
            .first()
            .await
            .unwrap()
            .unwrap()
            .tokenable_type,
        "Admin"
    );
}

#[tokio::test]
async fn tokens_may_be_revoked() {
    let _app = app().await;
    let taylor = taylor().await;
    let abigail = abigail().await;
    let laptop = taylor.create_token("laptop", &["*"]).await.unwrap();
    taylor.create_token("phone", &["*"]).await.unwrap();
    taylor.create_token("tablet", &["*"]).await.unwrap();
    abigail.create_token("laptop", &["*"]).await.unwrap();

    // Revoke a specific token...
    let deleted = taylor
        .tokens()
        .where_("id", laptop.access_token.id)
        .delete()
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert!(
        PersonalAccessToken::find_token(&laptop.plain_text_token)
            .await
            .unwrap()
            .is_none()
    );

    // Revoke all tokens...
    assert_eq!(taylor.tokens().delete().await.unwrap(), 2);
    assert_eq!(taylor.tokens().count().await.unwrap(), 0);
    assert_eq!(
        abigail.tokens().count().await.unwrap(),
        1,
        "other users keep theirs"
    );

    // A token model may delete itself, too.
    let mut token = abigail.tokens().first().await.unwrap().unwrap();
    assert!(token.delete().await.unwrap());
    assert_eq!(PersonalAccessToken::count().await.unwrap(), 0);
}

#[tokio::test]
async fn tokens_know_their_tokenable() {
    let _app = app().await;
    let user = taylor().await;
    let token = user.create_token("token-name", &["*"]).await.unwrap();

    let owner: User = token
        .access_token
        .tokenable::<User>()
        .get()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.email, "taylor@laravel.com");

    let owner = token.access_token.tokenable_user().await.unwrap().unwrap();
    assert_eq!(owner.id(), json!(user.id));
    assert!(owner.is::<User>());

    let mut orphan = token.access_token.clone();
    orphan.tokenable_type = "Unknown".into();
    assert!(orphan.tokenable_user().await.unwrap().is_none());
}
