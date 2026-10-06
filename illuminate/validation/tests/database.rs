//! The `unique`, `exists` and `current_password` rules.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::container;
use illuminate_support::{Result, Value, json};
use illuminate_validation::{
    ArrayPresenceVerifier, Condition, CurrentPasswordVerifier, PresenceVerifier, Rule, Rules,
    Validator, async_trait, rules,
};

fn verifier() -> Arc<ArrayPresenceVerifier> {
    Arc::new(
        ArrayPresenceVerifier::new()
            .with_table(
                "users",
                vec![
                    json!({"id": 1, "user_id": 10, "email": "taylor@laravel.com", "email_address": "t@l.com", "account_id": 1, "deleted_at": null}),
                    json!({"id": 2, "user_id": 20, "email": "abigail@laravel.com", "email_address": "a@l.com", "account_id": 2, "deleted_at": "2024-01-01"}),
                ],
            )
            .with_table(
                "states",
                vec![
                    json!({"state": "AR", "abbreviation": "AR", "name": "Arkansas"}),
                    json!({"state": "TX", "abbreviation": "TX", "name": "Texas"}),
                ],
            ),
    )
}

async fn run(data: Value, rules: impl Into<Rules>) -> Validator {
    let mut validator = Validator::make(data, rules).presence_verifier(verifier());
    validator.passes().await;
    validator
}

#[tokio::test]
async fn unique() {
    let _c = container();
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        [("email", "unique:users")],
    )
    .await;
    assert_eq!(
        validator.errors().first("email"),
        Some("The email has already been taken.")
    );
    let validator = run(
        json!({"email": "new@laravel.com"}),
        [("email", "unique:users")],
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"email": "t@l.com"}),
        [("email", "unique:users,email_address")],
    )
    .await;
    assert!(validator.errors().has("email"));
}

#[tokio::test]
async fn unique_can_ignore_a_row() {
    let _c = container();
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        [("email", "unique:users,email,1")],
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        [("email", "unique:users,email,2")],
    )
    .await;
    assert!(validator.errors().has("email"));
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        [("email", "unique:users,email,10,user_id")],
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"email": "taylor@laravel.com", "id": 1}),
        [("email", "unique:users,email,[id]")],
    )
    .await;
    assert!(validator.errors().is_empty());

    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        rules! { "email" => [Rule::unique("users", "email").ignore(1)] },
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        rules! { "email" => [Rule::unique("users", "").ignore(10).id_column("user_id")] },
    )
    .await;
    assert!(validator.errors().is_empty());
}

#[tokio::test]
async fn database_rules_support_extra_conditions() {
    let _c = container();
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        rules! { "email" => [Rule::unique("users", "email").where_("account_id", 2)] },
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        [("email", "unique:users,email,NULL,id,account_id,1")],
    )
    .await;
    assert!(validator.errors().has("email"));
    let validator = run(
        json!({"email": "abigail@laravel.com"}),
        rules! { "email" => [Rule::unique("users", "email").without_trashed()] },
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"email": "abigail@laravel.com"}),
        rules! { "email" => [Rule::unique("users", "email").where_not("account_id", 2)] },
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"email": "abigail@laravel.com"}),
        rules! { "email" => [Rule::unique("users", "email").where_in("account_id", [1, 2])] },
    )
    .await;
    assert!(validator.errors().has("email"));
    let validator = run(
        json!({"email": "abigail@laravel.com"}),
        rules! { "email" => [Rule::exists("users", "email").only_trashed()] },
    )
    .await;
    assert!(validator.errors().is_empty());
}

#[tokio::test]
async fn exists() {
    let _c = container();
    let validator = run(json!({"state": "AR"}), [("state", "exists:states")]).await;
    assert!(validator.errors().is_empty());
    let validator = run(json!({"state": "XX"}), [("state", "exists:states")]).await;
    assert_eq!(
        validator.errors().first("state"),
        Some("The selected state is invalid.")
    );
    let validator = run(
        json!({"code": "TX"}),
        [("code", "exists:states,abbreviation")],
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = run(
        json!({"code": "TX"}),
        rules! { "code" => [Rule::exists("states", "abbreviation").where_("name", "Arkansas")] },
    )
    .await;
    assert!(validator.errors().has("code"));
}

#[tokio::test]
async fn exists_checks_every_value_of_an_array() {
    let _c = container();
    let rules = || rules! { "states" => ["array", Rule::exists("states", "abbreviation")] };
    assert!(
        run(json!({"states": ["AR", "TX", "AR"]}), rules())
            .await
            .errors()
            .is_empty()
    );
    let validator = run(json!({"states": ["AR", "XX"]}), rules()).await;
    assert_eq!(
        validator.errors().first("states"),
        Some("The selected states is invalid.")
    );
}

#[tokio::test]
async fn wildcard_attributes_guess_their_column() {
    let _c = container();
    let validator = run(
        json!({"users": [{"email": "new@laravel.com"}, {"email": "taylor@laravel.com"}]}),
        [("users.*.email", "unique:users")],
    )
    .await;
    assert_eq!(validator.errors().keys(), vec!["users.1.email"]);
}

#[tokio::test]
async fn connections_prefix_tables() {
    let _c = container();
    let validator = run(
        json!({"email": "taylor@laravel.com"}),
        [("email", "unique:mysql.users,email")],
    )
    .await;
    assert!(validator.errors().has("email"));
}

struct Counting {
    calls: AtomicUsize,
}

#[async_trait]
impl PresenceVerifier for Counting {
    async fn count(
        &self,
        _: &str,
        _: &str,
        _: &Value,
        _: Option<&Value>,
        _: Option<&str>,
        _: &[Condition],
    ) -> Result<usize> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }

    async fn multi_count(
        &self,
        _: &str,
        _: &str,
        values: &[Value],
        _: &[Condition],
    ) -> Result<usize> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(values.len())
    }
}

#[tokio::test]
async fn presence_rules_are_skipped_after_failures_and_for_empty_values() {
    let (container, _guard) = container();
    let counting = Arc::new(Counting {
        calls: AtomicUsize::new(0),
    });
    container.instance_arc::<dyn PresenceVerifier>(counting.clone());

    let mut validator =
        Validator::make(json!({"email": "nope"}), [("email", "email|unique:users")]);
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().get("email"),
        vec!["The email field must be a valid email address."]
    );
    assert_eq!(counting.calls.load(Ordering::SeqCst), 0);

    assert!(
        Validator::make(json!({"email": ""}), [("email", "unique:users")])
            .passes()
            .await
    );
    assert_eq!(counting.calls.load(Ordering::SeqCst), 0);

    assert!(
        Validator::make(json!({"email": "a@b.com"}), [("email", "unique:users")])
            .passes()
            .await
    );
    assert_eq!(counting.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn presence_verifiers_can_be_set_on_the_factory() {
    let _c = container();
    Validator::set_presence_verifier(
        ArrayPresenceVerifier::new().with_table("users", vec![json!({"email": "x@y.z"})]),
    );
    assert!(
        !Validator::make(json!({"email": "x@y.z"}), [("email", "unique:users")])
            .passes()
            .await
    );
}

#[tokio::test]
async fn missing_presence_verifier_is_a_clear_error() {
    let _c = container();
    let mut validator = Validator::make(json!({"email": "x@y.z"}), [("email", "exists:users")]);
    assert_eq!(
        validator.try_passes().await.unwrap_err().to_string(),
        "Presence verifier has not been set."
    );
}

struct Passwords;

#[async_trait]
impl CurrentPasswordVerifier for Passwords {
    async fn check(&self, guard: Option<&str>, password: &str) -> bool {
        match guard {
            Some("api") => password == "api-secret",
            _ => password == "secret",
        }
    }
}

#[tokio::test]
async fn current_password() {
    let (container, _guard) = container();
    let mut validator = Validator::make(
        json!({"password": "secret"}),
        [("password", "current_password")],
    );
    assert!(validator.try_passes().await.is_err());

    container.instance_arc::<dyn CurrentPasswordVerifier>(Arc::new(Passwords));
    assert!(
        Validator::make(
            json!({"password": "secret"}),
            [("password", "current_password")]
        )
        .passes()
        .await
    );
    assert!(
        Validator::make(
            json!({"password": "api-secret"}),
            [("password", "current_password:api")]
        )
        .passes()
        .await
    );
    let mut validator = Validator::make(
        json!({"password": "wrong"}),
        [("password", "current_password")],
    );
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("password"),
        Some("The password is incorrect.")
    );
}
