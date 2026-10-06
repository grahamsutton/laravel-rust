//! The query builder against a live, in-memory SQLite database.

use illuminate_database::{
    Builder, Connection, MultipleRecordsFoundException, QueryException, RecordNotFoundException,
    RecordsNotFoundException, raw,
};
use illuminate_support::{Collection, Result, Value, ValueExt, json};
use serde::Deserialize;

fn connection() -> Connection {
    Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:"}),
    )
}

async fn seeded() -> Connection {
    let db = connection();
    db.get_schema_builder()
        .create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.integer("votes").default(0);
            table.boolean("admin").default(false);
            table.json("options").nullable();
            table.timestamps();
        })
        .await
        .unwrap();
    db.get_schema_builder()
        .create("posts", |table| {
            table.id();
            table.foreign_id("user_id").constrained();
            table.string("title");
            table.decimal("price", 8, 2).default(0);
        })
        .await
        .unwrap();
    db.table("users")
        .insert(json!([
            {"name": "Taylor", "email": "taylor@laravel.com", "votes": 10, "admin": true, "options": "{\"language\":\"en\",\"tags\":[\"a\",\"b\"]}", "created_at": "2024-01-15 10:00:00"},
            {"name": "Abigail", "email": "abigail@laravel.com", "votes": 5, "admin": false, "options": "{\"language\":\"fr\",\"tags\":[\"b\"]}", "created_at": "2024-02-20 11:30:00"},
            {"name": "James", "email": "james@laravel.com", "votes": 0, "admin": false, "options": null, "created_at": "2023-12-31 23:59:59"},
        ]))
        .await
        .unwrap();
    db.table("posts")
        .insert(json!([
            {"user_id": 1, "title": "Laravel", "price": 10.5},
            {"user_id": 1, "title": "Rust", "price": 20},
            {"user_id": 2, "title": "Eloquent", "price": 5.25},
        ]))
        .await
        .unwrap();
    db
}

#[derive(Debug, Deserialize, PartialEq)]
struct User {
    id: i64,
    name: String,
    email: String,
    votes: u32,
    admin: bool,
}

#[tokio::test]
async fn get_returns_rows_as_ordered_objects() {
    let db = seeded().await;
    let users = db.table("users").order_by("id", "asc").get().await.unwrap();
    assert_eq!(users.count(), 3);
    let first = users.first().unwrap();
    let keys: Vec<&String> = first.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec![
            "id",
            "name",
            "email",
            "votes",
            "admin",
            "options",
            "created_at",
            "updated_at"
        ]
    );
    assert_eq!(first["id"], json!(1));
    assert_eq!(first["name"], json!("Taylor"));
    assert_eq!(first["admin"], json!(1));
    assert_eq!(first["updated_at"], Value::Null);
}

#[tokio::test]
async fn get_as_deserializes_leniently() {
    let db = seeded().await;
    let users: Collection<User> = db
        .table("users")
        .order_by("id", "asc")
        .get_as()
        .await
        .unwrap();
    assert_eq!(
        users.first().unwrap(),
        &User {
            id: 1,
            name: "Taylor".into(),
            email: "taylor@laravel.com".into(),
            votes: 10,
            admin: true,
        }
    );
    let user: Option<User> = db
        .table("users")
        .where_("name", "James")
        .first_as()
        .await
        .unwrap();
    assert!(!user.unwrap().admin);
}

#[tokio::test]
async fn first_find_value_and_sole() {
    let db = seeded().await;
    let user = db
        .table("users")
        .where_("name", "Abigail")
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user["email"], json!("abigail@laravel.com"));

    assert!(
        db.table("users")
            .where_("name", "Nobody")
            .first()
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        db.table("users").find(3).await.unwrap().unwrap()["name"],
        json!("James")
    );

    assert_eq!(
        db.table("users")
            .where_("name", "Taylor")
            .value("email")
            .await
            .unwrap(),
        Some(json!("taylor@laravel.com"))
    );
    assert_eq!(
        db.table("users")
            .where_("id", 2)
            .value_as::<i64>("votes")
            .await
            .unwrap(),
        Some(5)
    );
    assert_eq!(
        db.table("users")
            .where_("id", 99)
            .value("votes")
            .await
            .unwrap(),
        None
    );

    let error = db
        .table("users")
        .where_("id", 99)
        .first_or_fail()
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<RecordNotFoundException>().is_some());

    assert_eq!(
        db.table("users").where_("id", 1).sole().await.unwrap()["name"],
        json!("Taylor")
    );
    let error = db.table("users").sole().await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<MultipleRecordsFoundException>()
            .unwrap()
            .count,
        2
    );
    let error = db.table("users").where_("id", 99).sole().await.unwrap_err();
    assert!(error.downcast_ref::<RecordsNotFoundException>().is_some());
    assert_eq!(
        db.table("users")
            .where_("id", 2)
            .sole_value("name")
            .await
            .unwrap(),
        json!("Abigail")
    );
}

#[tokio::test]
async fn pluck_and_implode() {
    let db = seeded().await;
    let names = db
        .table("users")
        .order_by("id", "asc")
        .pluck("name")
        .await
        .unwrap();
    assert_eq!(
        names.all(),
        &[json!("Taylor"), json!("Abigail"), json!("James")]
    );

    let by_email = db
        .table("users")
        .order_by("id", "asc")
        .pluck_with_key("name", "email")
        .await
        .unwrap();
    assert_eq!(by_email["abigail@laravel.com"], json!("Abigail"));
    assert_eq!(by_email.keys().next().unwrap(), "taylor@laravel.com");

    let titles = db
        .table("posts")
        .order_by("id", "asc")
        .pluck("posts.title")
        .await
        .unwrap();
    assert_eq!(titles.count(), 3);
    let aliased = db
        .table("users")
        .select("name as n")
        .order_by("id", "asc")
        .pluck("name as n")
        .await
        .unwrap();
    assert_eq!(aliased.first(), Some(&json!("Taylor")));

    assert_eq!(
        db.table("users")
            .order_by("id", "asc")
            .implode("name", ", ")
            .await
            .unwrap(),
        "Taylor, Abigail, James"
    );
}

#[tokio::test]
async fn aggregates() {
    let db = seeded().await;
    assert_eq!(db.table("users").count().await.unwrap(), 3);
    assert_eq!(
        db.table("users")
            .where_op("votes", ">", 1)
            .count()
            .await
            .unwrap(),
        2
    );
    assert_eq!(db.table("users").max("votes").await.unwrap(), json!(10));
    assert_eq!(db.table("users").min("votes").await.unwrap(), json!(0));
    assert_eq!(db.table("users").sum("votes").await.unwrap(), json!(15));
    assert_eq!(db.table("users").avg("votes").await.unwrap(), json!(5.0));
    assert_eq!(
        db.table("users")
            .where_("id", 99)
            .sum("votes")
            .await
            .unwrap(),
        json!(0)
    );
    assert_eq!(
        db.table("users")
            .where_("id", 99)
            .max("votes")
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        db.table("posts").sum("price").await.unwrap().to_f64_lossy(),
        Some(35.75)
    );
    assert_eq!(
        db.table("posts")
            .distinct()
            .count_column("user_id")
            .await
            .unwrap(),
        2
    );

    // Aggregates ignore selected columns and orders.
    assert_eq!(
        db.table("users")
            .select("name")
            .order_by("name", "asc")
            .count()
            .await
            .unwrap(),
        3
    );

    assert!(
        db.table("users")
            .where_("name", "Taylor")
            .exists()
            .await
            .unwrap()
    );
    assert!(
        db.table("users")
            .where_("name", "Nobody")
            .doesnt_exist()
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn pagination_count_handles_groups() {
    let db = seeded().await;
    assert_eq!(
        db.table("posts").get_count_for_pagination().await.unwrap(),
        3
    );
    assert_eq!(
        db.table("posts")
            .group_by("user_id")
            .get_count_for_pagination()
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        db.table("posts")
            .order_by("id", "asc")
            .for_page(2, 2)
            .get()
            .await
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn where_clauses_filter_rows() {
    let db = seeded().await;
    let count = |q: Builder| async move { q.count().await.unwrap() };

    assert_eq!(count(db.table("users").where_in("id", [1, 3])).await, 2);
    assert_eq!(count(db.table("users").where_not_in("id", [1, 3])).await, 1);
    assert_eq!(
        count(db.table("users").where_between("votes", [1, 10])).await,
        2
    );
    assert_eq!(count(db.table("users").where_null("updated_at")).await, 3);
    assert_eq!(count(db.table("users").where_not_null("options")).await, 2);
    assert_eq!(count(db.table("users").where_like("name", "%a%")).await, 3);
    assert_eq!(
        count(db.table("users").where_like_case_sensitive("name", "T%")).await,
        1
    );
    assert_eq!(
        count(db.table("users").where_like_case_sensitive("name", "t%")).await,
        0
    );
    assert_eq!(
        count(
            db.table("users")
                .where_any(["name", "email"], "like", "%abigail%")
        )
        .await,
        1
    );
    assert_eq!(
        count(
            db.table("users")
                .where_none(["name", "email"], "like", "%abigail%")
        )
        .await,
        2
    );
    assert_eq!(
        count(db.table("users").where_column_op("votes", ">", "admin")).await,
        2
    );
    assert_eq!(
        count(db.table("users").where_raw("votes * 2 > ?", (10,))).await,
        1
    );
    assert_eq!(
        count(
            db.table("users")
                .where_("admin", true)
                .or_where_group(|q| q.where_("votes", 5).where_("name", "Abigail"))
        )
        .await,
        2
    );
    assert_eq!(
        count(db.table("users").where_not(|q| q.where_("admin", true))).await,
        2
    );
}

#[tokio::test]
async fn date_and_json_wheres_run_on_sqlite() {
    let db = seeded().await;
    let count = |q: Builder| async move { q.count().await.unwrap() };

    assert_eq!(
        count(db.table("users").where_date("created_at", "2024-01-15")).await,
        1
    );
    assert_eq!(
        count(db.table("users").where_year("created_at", 2024)).await,
        2
    );
    assert_eq!(
        count(db.table("users").where_month("created_at", 2)).await,
        1
    );
    assert_eq!(
        count(db.table("users").where_day_op("created_at", ">", 15)).await,
        2
    );
    assert_eq!(
        count(db.table("users").where_time("created_at", "11:30:00")).await,
        1
    );

    assert_eq!(
        count(db.table("users").where_("options->language", "fr")).await,
        1
    );
    assert_eq!(
        count(db.table("users").where_json_contains("options->tags", "b")).await,
        2
    );
    assert_eq!(
        count(db.table("users").where_json_contains("options->tags", "a")).await,
        1
    );
    assert_eq!(
        count(
            db.table("users")
                .where_json_length_op("options->tags", ">", 1)
        )
        .await,
        1
    );
    assert_eq!(
        count(
            db.table("users")
                .where_json_contains_key("options->language")
        )
        .await,
        2
    );
}

#[tokio::test]
async fn sub_queries_execute() {
    let db = seeded().await;
    let authors = db
        .table("users")
        .where_in("id", |q: Builder| {
            q.select("user_id").from("posts").where_op("price", ">", 6)
        })
        .pluck("name")
        .await
        .unwrap();
    assert_eq!(authors.all(), &[json!("Taylor")]);

    let with_posts = db
        .table("users")
        .where_exists(|q: Builder| {
            q.select(raw("1"))
                .from("posts")
                .where_column("posts.user_id", "users.id")
        })
        .count()
        .await
        .unwrap();
    assert_eq!(with_posts, 2);

    let rows = db
        .table("users")
        .select(["name"])
        .select_sub(
            |q: Builder| {
                q.from("posts")
                    .select_raw("count(*)", ())
                    .where_column("posts.user_id", "users.id")
            },
            "post_count",
        )
        .order_by("id", "asc")
        .get()
        .await
        .unwrap();
    assert_eq!(rows[0], json!({"name": "Taylor", "post_count": 2}));
    assert_eq!(rows[2], json!({"name": "James", "post_count": 0}));
}

#[tokio::test]
async fn joins_and_groups_execute() {
    let db = seeded().await;
    let rows = db
        .table("users")
        .join("posts", "users.id", "=", "posts.user_id")
        .select(["users.name", "posts.title"])
        .order_by("posts.id", "asc")
        .get()
        .await
        .unwrap();
    assert_eq!(rows.count(), 3);
    assert_eq!(rows[1], json!({"name": "Taylor", "title": "Rust"}));

    let rows = db
        .table("users")
        .left_join("posts", "users.id", "=", "posts.user_id")
        .select(["users.name"])
        .select_raw("count(posts.id) as total", ())
        .group_by("users.name")
        .having_op("total", "<", 2)
        .order_by("users.name", "asc")
        .get()
        .await
        .unwrap();
    assert_eq!(
        rows.into_vec(),
        vec![
            json!({"name": "Abigail", "total": 1}),
            json!({"name": "James", "total": 0})
        ]
    );

    let latest = db
        .table("posts")
        .select("user_id")
        .select_raw("max(price) as top", ())
        .group_by("user_id");
    let rows = db
        .table("users")
        .join_sub(latest, "best", "users.id", "=", "best.user_id")
        .select(["users.name", "best.top"])
        .order_by("users.id", "asc")
        .get()
        .await
        .unwrap();
    assert_eq!(rows[0], json!({"name": "Taylor", "top": 20}));
}

#[tokio::test]
async fn unions_execute() {
    let db = seeded().await;
    let names = db
        .table("users")
        .select("name")
        .where_("id", 1)
        .union(db.table("users").select("name").where_("id", 2))
        .pluck("name")
        .await
        .unwrap();
    assert_eq!(names.count(), 2);
}

#[tokio::test]
async fn inserts_and_ids() {
    let db = seeded().await;
    let id = db
        .table("users")
        .insert_get_id(json!({"name": "Dayle", "email": "dayle@laravel.com"}))
        .await
        .unwrap();
    assert_eq!(id, 4);

    assert!(db.table("users").insert(json!([])).await.unwrap());
    assert_eq!(
        db.table("users")
            .insert_or_ignore(json!([
                {"name": "Dup", "email": "taylor@laravel.com"},
                {"name": "New", "email": "new@laravel.com"},
            ]))
            .await
            .unwrap(),
        1
    );
    assert_eq!(db.table("users").count().await.unwrap(), 5);

    db.get_schema_builder()
        .create("archive", |t| {
            t.string("name");
        })
        .await
        .unwrap();
    let copied = db
        .table("archive")
        .insert_using(
            &["name"],
            db.table("users").select("name").where_op("votes", ">", 1),
        )
        .await
        .unwrap();
    assert_eq!(copied, 2);
}

#[tokio::test]
async fn updates_and_upserts() {
    let db = seeded().await;
    let affected = db
        .table("users")
        .where_("id", 1)
        .update(json!({"votes": 1, "name": "Taylor Otwell"}))
        .await
        .unwrap();
    assert_eq!(affected, 1);
    assert_eq!(
        db.table("users").find(1).await.unwrap().unwrap()["name"],
        json!("Taylor Otwell")
    );

    // Updates with limits and joins become rowid sub-queries on SQLite.
    let affected = db
        .table("users")
        .order_by("id", "desc")
        .limit(1)
        .update(json!({"votes": 99}))
        .await
        .unwrap();
    assert_eq!(affected, 1);
    assert_eq!(
        db.table("users")
            .where_("id", 3)
            .value("votes")
            .await
            .unwrap(),
        Some(json!(99))
    );

    let affected = db
        .table("users")
        .join("posts", "users.id", "=", "posts.user_id")
        .where_("posts.title", "Eloquent")
        .update(json!({"users.votes": 42}))
        .await
        .unwrap();
    assert_eq!(affected, 1);
    assert_eq!(
        db.table("users")
            .where_("id", 2)
            .value("votes")
            .await
            .unwrap(),
        Some(json!(42))
    );

    // JSON columns can be patched in place.
    db.table("users")
        .where_("id", 2)
        .update(json!({"options->language": "de"}))
        .await
        .unwrap();
    assert_eq!(
        db.table("users")
            .where_("options->language", "de")
            .count()
            .await
            .unwrap(),
        1
    );

    let upserted = db
        .table("users")
        .upsert(
            json!([
                {"email": "taylor@laravel.com", "name": "T", "votes": 7},
                {"email": "jess@laravel.com", "name": "Jess", "votes": 3},
            ]),
            &["email"],
            Some(&["votes"]),
        )
        .await
        .unwrap();
    assert_eq!(upserted, 2);
    let taylor = db
        .table("users")
        .where_("email", "taylor@laravel.com")
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(taylor["votes"], json!(7));
    assert_eq!(taylor["name"], json!("Taylor Otwell"));
    assert_eq!(db.table("users").count().await.unwrap(), 4);

    assert!(
        db.table("users")
            .update_or_insert(
                json!({"email": "kelly@laravel.com"}),
                json!({"name": "Kelly"})
            )
            .await
            .unwrap()
    );
    assert_eq!(
        db.table("users")
            .where_("name", "Kelly")
            .count()
            .await
            .unwrap(),
        1
    );
    assert!(
        db.table("users")
            .update_or_insert(json!({"email": "kelly@laravel.com"}), json!({"votes": 8}))
            .await
            .unwrap()
    );
    assert_eq!(
        db.table("users")
            .where_("email", "kelly@laravel.com")
            .value("votes")
            .await
            .unwrap(),
        Some(json!(8))
    );
}

#[tokio::test]
async fn increments_and_decrements() {
    let db = seeded().await;
    db.table("users")
        .where_("id", 1)
        .increment("votes", 5)
        .await
        .unwrap();
    db.table("users")
        .where_("id", 1)
        .decrement("votes", 2)
        .await
        .unwrap();
    assert_eq!(
        db.table("users")
            .where_("id", 1)
            .value("votes")
            .await
            .unwrap(),
        Some(json!(13))
    );

    db.table("users")
        .where_("id", 2)
        .increment_with("votes", 1, json!({"name": "Abby"}))
        .await
        .unwrap();
    let abby = db.table("users").find(2).await.unwrap().unwrap();
    assert_eq!(
        (abby["votes"].clone(), abby["name"].clone()),
        (json!(6), json!("Abby"))
    );

    db.table("users")
        .increment_each([("votes", 1), ("admin", 1)])
        .await
        .unwrap();
    assert_eq!(db.table("users").sum("votes").await.unwrap(), json!(22));

    assert!(db.table("users").increment("votes", "lots").await.is_err());
}

#[tokio::test]
async fn deletes_and_truncates() {
    let db = seeded().await;
    assert_eq!(
        db.table("posts")
            .where_("user_id", 2)
            .delete()
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        db.table("posts")
            .order_by("id", "desc")
            .limit(1)
            .delete()
            .await
            .unwrap(),
        1
    );
    assert_eq!(db.table("posts").delete_by_id(1).await.unwrap(), 1);
    assert_eq!(db.table("posts").count().await.unwrap(), 0);

    db.table("users").truncate().await.unwrap();
    assert_eq!(db.table("users").count().await.unwrap(), 0);
    let id = db
        .table("users")
        .insert_get_id(json!({"name": "Fresh", "email": "fresh@laravel.com"}))
        .await
        .unwrap();
    assert_eq!(id, 1, "truncate resets the auto-increment sequence");
}

#[tokio::test]
async fn chunking() {
    let db = seeded().await;
    for i in 0..7 {
        db.table("posts")
            .insert(json!({"user_id": 3, "title": format!("Post {i}")}))
            .await
            .unwrap();
    }

    let mut seen = Vec::new();
    let completed = db
        .table("posts")
        .order_by("id", "asc")
        .chunk(4, |posts, page| {
            seen.push((page, posts.count()));
            async { Ok(true) }
        })
        .await
        .unwrap();
    assert!(completed);
    assert_eq!(seen, vec![(1, 4), (2, 4), (3, 2)]);

    let mut pages = 0;
    let completed = db
        .table("posts")
        .order_by("id", "asc")
        .chunk(4, |_, _| {
            pages += 1;
            async { Ok(false) }
        })
        .await
        .unwrap();
    assert!(!completed);
    assert_eq!(pages, 1);

    assert!(
        db.table("posts")
            .chunk(2, |_, _| async { Ok(true) })
            .await
            .is_err()
    );

    // chunk_by_id is safe while updating the chunked rows.
    let mut ids = Vec::new();
    let conn = db.clone();
    db.table("posts")
        .where_("user_id", 3)
        .chunk_by_id(3, |posts, _| {
            let conn = conn.clone();
            ids.extend(posts.iter().map(|p| p["id"].clone()));
            async move {
                for post in posts {
                    conn.table("posts")
                        .where_("id", post["id"].clone())
                        .update(json!({"user_id": 1}))
                        .await?;
                }
                Ok(true)
            }
        })
        .await
        .unwrap();
    assert_eq!(ids.len(), 7);
    assert_eq!(
        db.table("posts")
            .where_("user_id", 3)
            .count()
            .await
            .unwrap(),
        0
    );

    let mut titles = Vec::new();
    db.table("posts")
        .order_by("id", "asc")
        .limit(3)
        .each(2, |post| {
            titles.push(post["title"].clone());
            async { Ok(true) }
        })
        .await
        .unwrap();
    assert_eq!(titles.len(), 3);
}

#[tokio::test]
async fn query_exceptions_describe_the_query() {
    let db = connection();
    let error = db.table("missing").where_("id", 1).get().await.unwrap_err();
    let exception = error.downcast_ref::<QueryException>().unwrap();
    assert_eq!(exception.connection_name, "sqlite");
    assert_eq!(exception.sql, "select * from \"missing\" where \"id\" = ?");
    assert_eq!(exception.bindings, vec![json!(1)]);
    assert_eq!(
        exception.to_string(),
        "SQLSTATE[HY000]: General error: 1 no such table: missing (Connection: sqlite, SQL: select * from \"missing\" where \"id\" = 1)"
    );

    let db = seeded().await;
    let error = db
        .table("users")
        .insert(json!({"name": "Dup", "email": "taylor@laravel.com"}))
        .await
        .unwrap_err();
    let exception = error.downcast_ref::<QueryException>().unwrap();
    assert!(exception.is_unique_constraint_violation());
    assert!(exception.to_string().starts_with(
        "SQLSTATE[23000]: Integrity constraint violation: 19 UNIQUE constraint failed: users.email"
    ));
}

#[tokio::test]
async fn raw_queries() -> Result<()> {
    let db = seeded().await;
    let rows = db
        .select("select name from users where votes > ? order by id", (1,))
        .await?;
    assert_eq!(
        rows,
        vec![json!({"name": "Taylor"}), json!({"name": "Abigail"})]
    );
    assert_eq!(db.scalar("select count(*) from users", ()).await?, json!(3));
    assert!(db.scalar("select 1, 2", ()).await.is_err());
    assert_eq!(
        db.select_one("select * from users where id = ?", (99,))
            .await?,
        None
    );

    assert!(
        db.insert(
            "insert into users (name, email) values (?, ?)",
            ("Raw", "raw@laravel.com")
        )
        .await?
    );
    assert_eq!(
        db.update("update users set votes = ? where name = ?", (100, "Raw"))
            .await?,
        1
    );
    assert_eq!(
        db.delete("delete from users where name = ?", ["Raw"])
            .await?,
        1
    );
    assert!(db.statement("create table logs (message text)", ()).await?);
    assert!(
        db.unprepared("insert into logs values ('a'); insert into logs values ('b');")
            .await?
    );
    assert_eq!(db.table("logs").count().await?, 2);

    let row = db
        .select_one(
            "select ? as n, ? as f, ? as s, ? as missing, ? as yes",
            (1, 1.5, "x", Value::Null, true),
        )
        .await?;
    assert_eq!(
        row,
        Some(json!({"n": 1, "f": 1.5, "s": "x", "missing": null, "yes": 1}))
    );
    Ok(())
}

#[tokio::test]
async fn query_log_and_listeners() {
    let db = seeded().await;
    db.enable_query_log();
    db.table("users").where_("id", 1).first().await.unwrap();
    let log = db.get_query_log();
    assert_eq!(log.len(), 1);
    assert_eq!(
        log[0].query,
        "select * from \"users\" where \"id\" = ? limit 1"
    );
    assert_eq!(log[0].bindings, vec![json!(1)]);
    assert!(log[0].time.is_some());
    assert_eq!(
        db.get_raw_query_log()[0].query,
        "select * from \"users\" where \"id\" = 1 limit 1"
    );
    db.flush_query_log();
    assert!(db.get_query_log().is_empty());
    db.disable_query_log();
    db.table("users").get().await.unwrap();
    assert!(db.get_query_log().is_empty());

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = events.clone();
    db.listen(move |event| {
        captured.lock().unwrap().push((
            event.connection_name.clone(),
            event.sql.clone(),
            event.to_raw_sql(),
        ));
    });
    db.table("users")
        .where_("name", "Taylor")
        .count()
        .await
        .unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "sqlite");
    assert_eq!(
        events[0].2,
        "select count(*) as \"aggregate\" from \"users\" where \"name\" = 'Taylor'"
    );
}

#[tokio::test]
async fn pretending_runs_nothing() {
    let db = seeded().await;
    let queries = db
        .pretend(|| async {
            db.table("users").delete().await?;
            db.table("users")
                .insert(json!({"name": "X", "email": "x@x.com"}))
                .await?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        queries.iter().map(|q| q.query.as_str()).collect::<Vec<_>>(),
        vec![
            "delete from \"users\"",
            "insert into \"users\" (\"name\", \"email\") values ('X', 'x@x.com')",
        ]
    );
    assert!(queries.iter().all(|q| q.time.is_none()));
    assert_eq!(db.table("users").count().await.unwrap(), 3);
}

#[tokio::test]
async fn table_prefixes_are_applied() {
    let db = Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:", "prefix": "app_"}),
    );
    db.get_schema_builder()
        .create("users", |t| {
            t.id();
            t.string("name");
        })
        .await
        .unwrap();
    assert!(db.get_schema_builder().has_table("users").await.unwrap());
    db.table("users")
        .insert(json!({"name": "Taylor"}))
        .await
        .unwrap();
    assert_eq!(
        db.select("select name from app_users", ())
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        db.table("users as u")
            .where_("u.name", "Taylor")
            .count()
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn sqlite_file_databases_are_created_when_missing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("database.sqlite");
    let db = Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": path.to_string_lossy(), "foreign_key_constraints": true}),
    );
    db.statement("create table t (id integer)", ())
        .await
        .unwrap();
    assert!(path.exists());
    assert_eq!(
        db.scalar("pragma foreign_keys", ()).await.unwrap(),
        json!(1)
    );
    assert_eq!(db.get_database_name(), path.to_string_lossy());
    db.disconnect().await;
    db.table("t").insert(json!({"id": 1})).await.unwrap();
    assert_eq!(db.table("t").count().await.unwrap(), 1);
}

#[tokio::test]
async fn unconfigured_connections_fail_like_laravel() {
    let db = Connection::new("missing", Value::Null);
    let error = db.table("users").get().await.unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Database connection [missing] not configured.")
    );

    let db = Connection::new("weird", json!({"driver": "sqlsrv"}));
    assert!(
        db.table("users")
            .get()
            .await
            .unwrap_err()
            .to_string()
            .starts_with("Unsupported driver [sqlsrv].")
    );
}
