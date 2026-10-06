//! The schema builder against a live, in-memory SQLite database.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_database::schema::{IndexName, SchemaBuilder};
use illuminate_database::{Connection, DatabaseServiceProvider, QueryException, Schema};
use illuminate_support::json;

fn connection(foreign_keys: bool) -> Connection {
    Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:", "foreign_key_constraints": foreign_keys}),
    )
}

fn schema(foreign_keys: bool) -> SchemaBuilder {
    connection(foreign_keys).get_schema_builder()
}

async fn users_and_posts(schema: &SchemaBuilder) {
    schema
        .create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("posts", |table| {
            table.id();
            table
                .foreign_id("user_id")
                .constrained()
                .cascade_on_delete();
            table.string("title").index();
            table.text("body").nullable();
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn creating_and_inspecting_tables() {
    let schema = schema(true);
    users_and_posts(&schema).await;

    assert!(schema.has_table("users").await.unwrap());
    assert!(!schema.has_table("flights").await.unwrap());
    assert!(schema.has_column("users", "email").await.unwrap());
    assert!(schema.has_column("users", "EMAIL").await.unwrap());
    assert!(!schema.has_column("users", "password").await.unwrap());
    assert!(
        schema
            .has_columns("users", ["name", "email"])
            .await
            .unwrap()
    );
    assert!(
        !schema
            .has_columns("users", ["name", "password"])
            .await
            .unwrap()
    );

    assert_eq!(
        schema.get_column_listing("users").await.unwrap(),
        vec!["id", "name", "email", "created_at", "updated_at"]
    );
    assert_eq!(
        schema.get_table_listing().await.unwrap(),
        vec!["posts", "users"]
    );

    let columns = schema.get_columns("users").await.unwrap();
    let id = &columns[0];
    assert_eq!(id.type_name, "integer");
    assert!(id.auto_increment);
    assert!(!id.nullable);
    assert!(columns[3].nullable);
    assert_eq!(
        schema.get_column_type("users", "name").await.unwrap(),
        "varchar"
    );
    assert!(schema.get_column_type("users", "missing").await.is_err());

    let indexes = schema.get_indexes("users").await.unwrap();
    assert!(
        indexes
            .iter()
            .any(|i| i.name == "users_email_unique" && i.unique && i.columns == vec!["email"])
    );
    assert!(indexes.iter().any(|i| i.primary && i.columns == vec!["id"]));
    assert!(
        schema
            .has_index("users", "users_email_unique", Some("unique"))
            .await
            .unwrap()
    );
    assert!(
        schema
            .has_index("posts", IndexName::Columns(vec!["title".into()]), None)
            .await
            .unwrap()
    );
    assert!(
        !schema
            .has_index("posts", "posts_body_index", None)
            .await
            .unwrap()
    );

    let foreign_keys = schema.get_foreign_keys("posts").await.unwrap();
    assert_eq!(foreign_keys.len(), 1);
    assert_eq!(foreign_keys[0].columns, vec!["user_id"]);
    assert_eq!(foreign_keys[0].foreign_table, "users");
    assert_eq!(foreign_keys[0].foreign_columns, vec!["id"]);
    assert_eq!(foreign_keys[0].on_delete, "cascade");
    assert!(
        schema
            .has_foreign_key("posts", vec!["user_id"])
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn foreign_keys_are_enforced_when_configured() {
    let schema = schema(true);
    users_and_posts(&schema).await;
    let db = schema.get_connection().clone();

    let error = db
        .table("posts")
        .insert(json!({"user_id": 99, "title": "Orphan"}))
        .await
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<QueryException>()
            .unwrap()
            .to_string()
            .contains("FOREIGN KEY constraint failed")
    );

    schema
        .without_foreign_key_constraints(|| async {
            db.table("posts")
                .insert(json!({"user_id": 99, "title": "Orphan"}))
                .await?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(db.table("posts").count().await.unwrap(), 1);

    // Cascading deletes.
    let user = db
        .table("users")
        .insert_get_id(json!({"name": "Taylor", "email": "t@laravel.com"}))
        .await
        .unwrap();
    db.table("posts")
        .insert(json!({"user_id": user, "title": "Hello"}))
        .await
        .unwrap();
    db.table("users").delete_by_id(user).await.unwrap();
    assert_eq!(
        db.table("posts")
            .where_("user_id", user)
            .count()
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn altering_tables() {
    let schema = schema(false);
    users_and_posts(&schema).await;
    let db = schema.get_connection().clone();
    db.table("users")
        .insert(json!({"name": "Taylor", "email": "t@laravel.com"}))
        .await
        .unwrap();

    schema
        .table("users", |table| {
            table.integer("votes").default(0);
            table.string("nickname").nullable().unique();
            table.rename_column("name", "full_name");
        })
        .await
        .unwrap();
    assert_eq!(
        schema.get_column_listing("users").await.unwrap(),
        vec![
            "id",
            "full_name",
            "email",
            "created_at",
            "updated_at",
            "votes",
            "nickname"
        ]
    );
    assert_eq!(
        db.table("users").value("votes").await.unwrap(),
        Some(json!(0))
    );
    assert!(
        schema
            .has_index("users", "users_nickname_unique", None)
            .await
            .unwrap()
    );

    schema
        .table("users", |table| {
            table.drop_unique(vec!["nickname"]);
            table.drop_column("nickname");
        })
        .await
        .unwrap();
    assert!(!schema.has_column("users", "nickname").await.unwrap());

    schema.drop_columns("users", ["votes"]).await.unwrap();
    assert!(!schema.has_column("users", "votes").await.unwrap());

    schema.rename("users", "people").await.unwrap();
    assert!(schema.has_table("people").await.unwrap());
    assert!(!schema.has_table("users").await.unwrap());

    schema.drop("posts").await.unwrap();
    schema.drop_if_exists("posts").await.unwrap();
    assert!(schema.drop("posts").await.is_err());
}

#[tokio::test]
async fn sqlite_rebuilds_tables_for_foreign_keys_and_changes() {
    let schema = schema(true);
    users_and_posts(&schema).await;
    let db = schema.get_connection().clone();
    let user = db
        .table("users")
        .insert_get_id(json!({"name": "Taylor", "email": "t@laravel.com"}))
        .await
        .unwrap();
    db.table("posts")
        .insert(json!({"user_id": user, "title": "Hello"}))
        .await
        .unwrap();

    // Adding a constrained column to an existing table rebuilds it.
    schema
        .create("teams", |table| {
            table.id();
            table.string("name");
        })
        .await
        .unwrap();
    schema
        .table("posts", |table| {
            table
                .foreign_id("team_id")
                .nullable()
                .constrained()
                .null_on_delete();
        })
        .await
        .unwrap();
    let foreign_keys = schema.get_foreign_keys("posts").await.unwrap();
    assert_eq!(foreign_keys.len(), 2);
    assert!(
        foreign_keys
            .iter()
            .any(|fk| fk.foreign_table == "teams" && fk.on_delete == "set null")
    );

    // The data, indexes and auto-increment primary key survived.
    assert_eq!(
        db.table("posts").value("title").await.unwrap(),
        Some(json!("Hello"))
    );
    assert!(
        schema
            .has_index("posts", "posts_title_index", None)
            .await
            .unwrap()
    );
    assert!(schema.get_columns("posts").await.unwrap()[0].auto_increment);
    assert_eq!(
        db.scalar("pragma foreign_keys", ()).await.unwrap(),
        json!(1)
    );

    // Changing a column.
    schema
        .table("posts", |table| {
            table.string("title").nullable().change();
        })
        .await
        .unwrap();
    let title = schema
        .get_columns("posts")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name == "title")
        .unwrap();
    assert!(title.nullable);

    // Dropping a constrained foreign id.
    schema
        .table("posts", |table| {
            table.drop_constrained_foreign_id("team_id");
        })
        .await
        .unwrap();
    assert!(!schema.has_column("posts", "team_id").await.unwrap());
    assert_eq!(schema.get_foreign_keys("posts").await.unwrap().len(), 1);

    // Renaming an index.
    schema
        .table("posts", |table| {
            table.rename_index("posts_title_index", "posts_headline_index");
        })
        .await
        .unwrap();
    assert!(
        schema
            .has_index("posts", "posts_headline_index", None)
            .await
            .unwrap()
    );
    assert!(
        !schema
            .has_index("posts", "posts_title_index", None)
            .await
            .unwrap()
    );

    assert!(
        schema
            .table("posts", |table| table.drop_foreign("posts_user_id_foreign"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn dropping_all_tables() {
    let schema = schema(true);
    users_and_posts(&schema).await;
    schema
        .get_connection()
        .statement("create view titles as select title from posts", ())
        .await
        .unwrap();

    schema.drop_all_views().await.unwrap();
    schema.drop_all_tables().await.unwrap();
    assert!(schema.get_table_listing().await.unwrap().is_empty());

    // Tables can be created again afterwards.
    users_and_posts(&schema).await;
    assert_eq!(schema.get_table_listing().await.unwrap().len(), 2);
}

#[tokio::test]
async fn column_defaults_and_types_round_trip() {
    let schema = schema(false);
    schema
        .create("flights", |table| {
            table.id();
            table.string("name").default("Untitled");
            table.boolean("active").default(true);
            table.decimal("price", 8, 2).default(9.99);
            table
                .enum_("status", ["scheduled", "departed"])
                .default("scheduled");
            table.json("meta").nullable();
            table.timestamp("departs_at").use_current();
            table.uuid("uuid").nullable();
        })
        .await
        .unwrap();
    let db = schema.get_connection().clone();
    db.table("flights")
        .insert(json!({"meta": "{\"gate\":12}"}))
        .await
        .unwrap();
    let flight = db.table("flights").first().await.unwrap().unwrap();
    assert_eq!(flight["name"], json!("Untitled"));
    assert_eq!(flight["active"], json!(1));
    assert_eq!(flight["price"], json!(9.99));
    assert_eq!(flight["status"], json!("scheduled"));
    assert!(flight["departs_at"].as_str().unwrap().len() >= 19);

    let error = db
        .table("flights")
        .insert(json!({"status": "lost"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("CHECK constraint failed"));
}

#[tokio::test]
async fn the_schema_facade_uses_the_default_connection() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({"database": {
        "default": "sqlite",
        "connections": {
            "sqlite": {"driver": "sqlite", "database": ":memory:"},
            "secondary": {"driver": "sqlite", "database": ":memory:"},
        },
    }})));
    DatabaseServiceProvider.register(&container);

    Schema::create("flights", |table| {
        table.id();
        table.string("name");
    })
    .await
    .unwrap();
    assert!(Schema::has_table("flights").await.unwrap());
    assert!(Schema::has_column("flights", "name").await.unwrap());
    assert_eq!(
        Schema::get_column_listing("flights").await.unwrap(),
        vec!["id", "name"]
    );
    assert_eq!(Schema::get_tables().await.unwrap()[0].name, "flights");

    assert!(
        !Schema::connection("secondary")
            .has_table("flights")
            .await
            .unwrap()
    );
    Schema::connection("secondary")
        .create("logs", |table| {
            table.text("message");
        })
        .await
        .unwrap();
    assert!(!Schema::has_table("logs").await.unwrap());

    Schema::table("flights", |table| {
        table.string("airline").nullable();
    })
    .await
    .unwrap();
    assert!(
        Schema::has_columns("flights", ["name", "airline"])
            .await
            .unwrap()
    );
    Schema::rename("flights", "trips").await.unwrap();
    Schema::drop_if_exists("trips").await.unwrap();
    assert!(Schema::get_table_listing().await.unwrap().is_empty());
}
