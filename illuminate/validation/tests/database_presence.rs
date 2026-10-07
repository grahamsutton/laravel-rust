//! The `unique` and `exists` rules against a real (in-memory SQLite)
//! database, through the `DatabasePresenceVerifier`.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_database::{DB, DatabaseManager, DatabaseServiceProvider, Schema};
use illuminate_support::{Value, json};
use illuminate_validation::{
    ArrayPresenceVerifier, Condition, DatabasePresenceVerifier, Factory, PresenceVerifier, Rule,
    ValidationServiceProvider, Validator, rules,
};

fn config() -> Repository {
    Repository::new(json!({
        "database": {
            "default": "sqlite",
            "connections": {
                "sqlite": {"driver": "sqlite", "database": ":memory:"},
                "secondary": {"driver": "sqlite", "database": ":memory:"},
            },
        },
    }))
}

/// Boot an application with the database and validation providers, and
/// seed the `users` and `states` tables.
async fn app() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(config());
    DatabaseServiceProvider.register(&container);
    ValidationServiceProvider.register(&container);
    ValidationServiceProvider.boot(&container);
    seed().await;
    (container, guard)
}

async fn seed() {
    Schema::create("users", |table| {
        table.id();
        table.string("email");
        table.string("email_address").nullable();
        table.integer("account_id");
        table.string("status").nullable();
        table.timestamp("deleted_at").nullable();
    })
    .await
    .unwrap();
    DB::table("users")
        .insert(json!([
            {"email": "taylor@laravel.com", "email_address": "t@l.com", "account_id": 1, "status": "active", "deleted_at": null},
            {"email": "abigail@laravel.com", "email_address": null, "account_id": 2, "status": "banned", "deleted_at": "2024-01-01 00:00:00"},
            {"email": "taylor@laravel.com", "email_address": null, "account_id": 2, "status": null, "deleted_at": null},
        ]))
        .await
        .unwrap();

    for connection in ["sqlite", "secondary"] {
        Schema::connection(connection)
            .create("states", |table| {
                table.id();
                table.string("abbreviation");
                table.string("name");
            })
            .await
            .unwrap();
    }
    DB::table("states")
        .insert(json!([
            {"abbreviation": "AR", "name": "Arkansas"},
            {"abbreviation": "TX", "name": "Texas"},
            {"abbreviation": "TX", "name": "Texas (duplicate)"},
        ]))
        .await
        .unwrap();
    DB::connection("secondary")
        .table("states")
        .insert(json!({"abbreviation": "CA", "name": "California"}))
        .await
        .unwrap();
}

fn verifier() -> DatabasePresenceVerifier {
    DatabasePresenceVerifier::resolve()
}

async fn count(column: &str, value: Value, exclude: Option<Value>, extra: &[Condition]) -> usize {
    verifier()
        .count("users", column, &value, exclude.as_ref(), None, extra)
        .await
        .unwrap()
}

fn taylor() -> Value {
    json!("taylor@laravel.com")
}

#[tokio::test]
async fn it_counts_matching_rows() {
    let _app = app().await;
    assert_eq!(count("email", taylor(), None, &[]).await, 2);
    assert_eq!(
        count("email", json!("nobody@laravel.com"), None, &[]).await,
        0
    );
    assert_eq!(count("account_id", json!("2"), None, &[]).await, 2);
}

#[tokio::test]
async fn it_excludes_the_ignored_row() {
    let _app = app().await;
    assert_eq!(count("email", taylor(), Some(json!(1)), &[]).await, 1);
    assert_eq!(count("email", taylor(), Some(json!("3")), &[]).await, 1);
    assert_eq!(count("email", taylor(), Some(json!("NULL")), &[]).await, 2);
    assert_eq!(count("email", taylor(), Some(Value::Null), &[]).await, 2);

    // A custom ID column.
    let verifier = verifier();
    let count = verifier
        .count(
            "users",
            "email",
            &taylor(),
            Some(&json!(1)),
            Some("account_id"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn extra_conditions_follow_laravels_conventions() {
    let _app = app().await;
    let column = |c: &str| c.to_string();

    let cases: Vec<(Vec<Condition>, usize)> = vec![
        (vec![Condition::Where(column("account_id"), json!("1"))], 1),
        (vec![Condition::Where(column("account_id"), json!(2))], 1),
        (vec![Condition::Where(column("status"), json!("NULL"))], 1),
        (vec![Condition::Where(column("status"), Value::Null)], 1),
        (
            vec![Condition::Where(column("status"), json!("NOT_NULL"))],
            1,
        ),
        (vec![Condition::Where(column("account_id"), json!("!1"))], 1),
        (vec![Condition::WhereNot(column("account_id"), json!(1))], 1),
        (vec![Condition::WhereNot(column("status"), Value::Null)], 1),
        (vec![Condition::WhereNull(column("deleted_at"))], 2),
        (vec![Condition::WhereNotNull(column("email_address"))], 1),
        (
            vec![Condition::WhereIn(
                column("account_id"),
                vec![json!(1), json!(3)],
            )],
            1,
        ),
        (
            vec![Condition::WhereNotIn(column("account_id"), vec![json!(1)])],
            1,
        ),
        (
            vec![
                Condition::Where(column("account_id"), json!("2")),
                Condition::WhereNull(column("status")),
            ],
            1,
        ),
        (
            vec![
                Condition::Where(column("account_id"), json!("1")),
                Condition::WhereNull(column("status")),
            ],
            0,
        ),
    ];
    for (conditions, expected) in cases {
        assert_eq!(
            count("email", taylor(), None, &conditions).await,
            expected,
            "{conditions:?}"
        );
    }
}

#[tokio::test]
async fn multi_count_counts_distinct_values() {
    let _app = app().await;
    let verifier = verifier();
    let multi = |values: Vec<Value>, extra: Vec<Condition>| {
        let verifier = &verifier;
        async move {
            verifier
                .multi_count("states", "abbreviation", &values, &extra)
                .await
                .unwrap()
        }
    };
    // "TX" appears twice but counts once.
    assert_eq!(multi(vec![json!("AR"), json!("TX")], vec![]).await, 2);
    assert_eq!(multi(vec![json!("AR"), json!("NY")], vec![]).await, 1);
    assert_eq!(
        multi(
            vec![json!("AR"), json!("TX")],
            vec![Condition::Where("name".into(), json!("Texas"))]
        )
        .await,
        1
    );
}

#[tokio::test]
async fn tables_may_name_their_connection() {
    let _app = app().await;
    let verifier = verifier();
    let count = |table: &'static str, value: &'static str| {
        let verifier = &verifier;
        async move {
            verifier
                .count(table, "abbreviation", &json!(value), None, None, &[])
                .await
                .unwrap()
        }
    };
    assert_eq!(count("secondary.states", "CA").await, 1);
    assert_eq!(count("secondary.states", "AR").await, 0);
    assert_eq!(count("states", "CA").await, 0);
    // Not a connection: the name is used as a (schema-qualified) table.
    assert_eq!(count("main.states", "AR").await, 1);

    verifier.set_connection(Some("secondary"));
    assert_eq!(verifier.get_connection_name().as_deref(), Some("secondary"));
    assert_eq!(count("states", "CA").await, 1);
    assert_eq!(count("sqlite.states", "AR").await, 1);
    verifier.set_connection(None);
    assert_eq!(count("states", "CA").await, 0);
}

#[tokio::test]
async fn query_errors_are_reported() {
    let _app = app().await;
    let error = verifier()
        .count("missing", "email", &taylor(), None, None, &[])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no such table"), "{error}");
}

#[tokio::test]
async fn the_unique_rule_queries_the_database() {
    let _app = app().await;

    let error = Validator::make(
        json!({"email": "taylor@laravel.com"}),
        rules! {"email" => "required|email|unique:users"},
    )
    .validate()
    .await
    .unwrap_err();
    assert_eq!(
        error.errors()["email"],
        vec!["The email has already been taken.".to_string()]
    );

    let validated = Validator::make(
        json!({"email": "new@laravel.com", "extra": true}),
        rules! {"email" => "required|email|unique:users,email"},
    )
    .validate()
    .await
    .unwrap();
    assert_eq!(validated, json!({"email": "new@laravel.com"}));

    // A different column.
    assert!(
        Validator::make(
            json!({"email": "t@l.com"}),
            rules! {"email" => "unique:users,email_address"}
        )
        .validate()
        .await
        .is_err()
    );
}

#[tokio::test]
async fn the_unique_rule_can_ignore_rows_and_scope_queries() {
    let _app = app().await;
    let check = |data: Value, rules: illuminate_validation::Rules| async move {
        Validator::make(data, rules).validate().await.is_ok()
    };
    let taylor = json!({"email": "taylor@laravel.com"});

    // String rules: ignore an ID, and add `column,value` conditions.
    assert!(!check(taylor.clone(), rules! {"email" => "unique:users,email,1"}).await);
    assert!(
        check(
            taylor.clone(),
            rules! {"email" => "unique:users,email,1,id,account_id,1"}
        )
        .await
    );
    assert!(
        check(
            taylor.clone(),
            rules! {"email" => "unique:users,email,NULL,id,account_id,3"}
        )
        .await
    );
    assert!(
        !check(
            taylor.clone(),
            rules! {"email" => "unique:users,email,NULL,id,status,NULL"}
        )
        .await
    );
    assert!(
        check(
            taylor.clone(),
            rules! {"email" => "unique:users,email,NULL,id,status,banned"}
        )
        .await
    );
    assert!(
        !check(
            taylor.clone(),
            rules! {"email" => "unique:users,email,NULL,id,status,!banned"}
        )
        .await
    );

    // Rule objects.
    assert!(
        !check(
            taylor.clone(),
            rules! {"email" => [Rule::unique("users", "email").ignore(1)]}
        )
        .await
    );
    assert!(
        check(
            taylor.clone(),
            rules! {"email" => [Rule::unique("users", "email").ignore(1).where_("account_id", 1)]}
        )
        .await
    );
    assert!(
        check(
            json!({"email": "abigail@laravel.com"}),
            rules! {"email" => [Rule::unique("users", "email").without_trashed()]}
        )
        .await
    );
    assert!(
        !check(
            json!({"email": "abigail@laravel.com"}),
            rules! {"email" => [Rule::unique("users", "email").only_trashed()]}
        )
        .await
    );
    assert!(
        check(
            taylor.clone(),
            rules! {"email" => [Rule::unique("users", "email").where_in("account_id", [3, 4])]}
        )
        .await
    );
    assert!(
        check(
            taylor.clone(),
            rules! {"email" => [Rule::unique("users", "email").where_not("account_id", 1).where_not("account_id", 2)]}
        )
        .await
    );

    // Ignoring the row whose ID is in another field (`[field]`).
    assert!(
        check(
            json!({"email": "taylor@laravel.com", "user_id": 3}),
            rules! {"email" => "unique:users,email,[user_id],id,account_id,2"}
        )
        .await
    );
    assert!(
        !check(
            json!({"email": "taylor@laravel.com", "user_id": 1}),
            rules! {"email" => "unique:users,email,[user_id],id,account_id,2"}
        )
        .await
    );
}

#[tokio::test]
async fn the_exists_rule_queries_the_database() {
    let _app = app().await;

    let validated = Validator::make(
        json!({"state": "AR"}),
        rules! {"state" => "exists:states,abbreviation"},
    )
    .validate()
    .await
    .unwrap();
    assert_eq!(validated, json!({"state": "AR"}));

    let error = Validator::make(
        json!({"state": "NY"}),
        rules! {"state" => "exists:states,abbreviation"},
    )
    .validate()
    .await
    .unwrap_err();
    assert_eq!(
        error.errors()["state"],
        vec!["The selected state is invalid.".to_string()]
    );

    // The column defaults to the attribute's name.
    assert!(
        Validator::make(
            json!({"abbreviation": "TX"}),
            rules! {"abbreviation" => "exists:states"}
        )
        .validate()
        .await
        .is_ok()
    );

    // Arrays: every (distinct) value must exist.
    assert!(
        Validator::make(
            json!({"states": ["AR", "TX", "TX"]}),
            rules! {"states" => "array|exists:states,abbreviation"}
        )
        .validate()
        .await
        .is_ok()
    );
    assert!(
        Validator::make(
            json!({"states": ["AR", "NY"]}),
            rules! {"states" => "array|exists:states,abbreviation"}
        )
        .validate()
        .await
        .is_err()
    );
    assert!(
        Validator::make(
            json!({"states": ["AR", "NY"]}),
            rules! {"states.*" => "exists:states,abbreviation"}
        )
        .validate()
        .await
        .is_err()
    );

    // Conditions, rule objects and other connections.
    assert!(
        Validator::make(
            json!({"state": "TX"}),
            rules! {"state" => "exists:states,abbreviation,name,Texas"}
        )
        .validate()
        .await
        .is_ok()
    );
    assert!(
        Validator::make(
            json!({"state": "AR"}),
            rules! {"state" => [Rule::exists("states", "abbreviation").where_("name", "Texas")]}
        )
        .validate()
        .await
        .is_err()
    );
    assert!(
        Validator::make(
            json!({"state": "CA"}),
            rules! {"state" => "exists:secondary.states,abbreviation"}
        )
        .validate()
        .await
        .is_ok()
    );
    assert!(
        Validator::make(
            json!({"state": "CA"}),
            rules! {"state" => "exists:states,abbreviation"}
        )
        .validate()
        .await
        .is_err()
    );
}

#[tokio::test]
async fn the_provider_binds_the_database_verifier() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(config());

    // Without a database, there is no verifier.
    ValidationServiceProvider.register(&container);
    ValidationServiceProvider.boot(&container);
    assert!(!container.bound::<dyn PresenceVerifier>());
    assert!(container.make::<Factory>().presence_verifier().is_none());

    // The database registered after validation is picked up when booting.
    DatabaseServiceProvider.register(&container);
    assert!(container.make::<Factory>().presence_verifier().is_some());
    ValidationServiceProvider.boot(&container);
    assert!(container.bound::<dyn PresenceVerifier>());
    seed().await;
    assert!(
        Validator::make(
            json!({"state": "TX"}),
            rules! {"state" => "exists:states,abbreviation"}
        )
        .validate()
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn a_custom_verifier_is_never_replaced() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(config());
    container.instance_arc::<dyn PresenceVerifier>(Arc::new(
        ArrayPresenceVerifier::new().with_table("states", vec![json!({"abbreviation": "ZZ"})]),
    ));
    DatabaseServiceProvider.register(&container);
    ValidationServiceProvider.register(&container);
    ValidationServiceProvider.boot(&container);

    assert!(
        Validator::make(
            json!({"state": "ZZ"}),
            rules! {"state" => "exists:states,abbreviation"}
        )
        .validate()
        .await
        .is_ok()
    );
    assert!(container.bound::<DatabaseManager>());
}
