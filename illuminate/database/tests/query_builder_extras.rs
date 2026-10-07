//! The query builder's vector, JSON, row value, lateral join, index hint,
//! group limit, callback, cursor and pagination features.

use futures::StreamExt;
use illuminate_database::pagination::{Cursor, CursorPaginator};
use illuminate_database::query::FullTextOptions;
use illuminate_database::{Builder, Connection, Expression, raw};
use illuminate_support::error::{InvalidArgumentException, LogicException, RuntimeException};
use illuminate_support::{Collection, Value, json};

fn sqlite() -> Connection {
    Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:"}),
    )
}

fn mysql() -> Connection {
    Connection::new("mysql", json!({"driver": "mysql", "database": "laravel"}))
}

fn mariadb() -> Connection {
    Connection::new(
        "mariadb",
        json!({"driver": "mariadb", "database": "laravel"}),
    )
}

fn pgsql() -> Connection {
    Connection::new("pgsql", json!({"driver": "pgsql", "database": "laravel"}))
}

fn error_of(query: &Builder) -> String {
    query.try_to_sql().unwrap_err().to_string()
}

async fn seeded() -> Connection {
    let db = sqlite();
    db.get_schema_builder()
        .create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.integer("votes").default(0);
            table.integer("min_votes").default(0);
            table.integer("max_votes").default(100);
            table.json("options").nullable();
        })
        .await
        .unwrap();
    db.table("users")
        .insert(json!([
            {"name": "Taylor", "email": "taylor@laravel.com", "votes": 10, "options": "{\"languages\":[\"en\",\"fr\"],\"theme\":\"dark\"}"},
            {"name": "Abigail", "email": "abigail@laravel.com", "votes": 5, "options": "{\"languages\":[\"de\"]}"},
            {"name": "James", "email": "james@laravel.com", "votes": 5, "options": null},
            {"name": "Dayle", "email": "dayle@laravel.com", "votes": 20, "options": null},
            {"name": "Jess", "email": "jess@laravel.com", "votes": 1, "options": null},
        ]))
        .await
        .unwrap();
    db
}

// ----------------------------------------------------------------------
// Vector similarity
// ----------------------------------------------------------------------

#[test]
fn vector_similarity_clauses_on_postgres() {
    let query =
        pgsql()
            .table("documents")
            .where_vector_similar_to("embedding", vec![0.1, 0.2, 0.3], 0.4);
    assert_eq!(
        query.to_sql(),
        "select * from \"documents\" where (\"embedding\" <=> ?) <= ? order by (\"embedding\" <=> ?) asc"
    );
    assert_eq!(
        query.get_bindings(),
        vec![json!("[0.1,0.2,0.3]"), json!(0.6), json!("[0.1,0.2,0.3]")]
    );

    let query = pgsql().table("documents").where_vector_similar_to_with(
        "embedding",
        [0.5_f32, 1.0],
        0.6,
        false,
    );
    assert_eq!(
        query.to_sql(),
        "select * from \"documents\" where (\"embedding\" <=> ?) <= ?"
    );
    assert_eq!(query.get_bindings()[0], json!("[0.5,1.0]"));

    let query = pgsql()
        .table("documents")
        .select(["id", "title"])
        .select_vector_distance("embedding", vec![1.0, 0.0], None)
        .where_vector_distance_less_than("embedding", vec![1.0, 0.0], 0.3)
        .or_where_vector_distance_less_than("summary_embedding", vec![1.0, 0.0], 0.2)
        .order_by_vector_distance("embedding", vec![1.0, 0.0]);
    assert_eq!(
        query.to_sql(),
        "select \"id\", \"title\", (\"embedding\" <=> ?) as \"embedding_distance\" from \"documents\" where (\"embedding\" <=> ?) <= ? or (\"summary_embedding\" <=> ?) <= ? order by (\"embedding\" <=> ?) asc"
    );
    assert_eq!(
        query.get_bindings(),
        vec![
            json!("[1.0,0.0]"),
            json!("[1.0,0.0]"),
            json!(0.3),
            json!("[1.0,0.0]"),
            json!(0.2),
            json!("[1.0,0.0]")
        ]
    );
}

#[test]
fn vector_similarity_clauses_on_mariadb() {
    let query = mariadb()
        .table("documents")
        .select_vector_distance("embedding", vec![0.1], Some("distance"))
        .where_vector_similar_to("embedding", vec![0.1], 0.6);
    assert_eq!(
        query.to_sql(),
        "select vec_distance_cosine(`embedding`, vec_fromtext(?)) as `distance` from `documents` where vec_distance_cosine(`embedding`, vec_fromtext(?)) <= ? order by vec_distance_cosine(`embedding`, vec_fromtext(?)) asc"
    );
}

#[test]
fn vector_queries_fail_on_unsupported_drivers() {
    for connection in [sqlite(), mysql()] {
        let query =
            connection
                .table("documents")
                .where_vector_similar_to("embedding", vec![0.1], 0.6);
        let error = query.try_to_sql().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Vector distance queries are only supported by Postgres and MariaDB."
        );
        assert!(error.downcast_ref::<RuntimeException>().is_some());
    }
    let query = sqlite()
        .table("documents")
        .select_vector_distance("embedding", vec![0.1], None);
    assert!(query.try_to_sql().is_err());
    let query = sqlite()
        .table("documents")
        .order_by_vector_distance("embedding", json!([0.1]));
    assert!(query.try_to_sql().is_err());
}

// ----------------------------------------------------------------------
// Where clauses
// ----------------------------------------------------------------------

#[test]
fn full_text_clauses() {
    let query = mysql()
        .table("posts")
        .where_full_text("title", "Laravel")
        .or_where_full_text(["title", "body"], "Rust");
    assert_eq!(
        query.to_sql(),
        "select * from `posts` where match (`title`) against (? in natural language mode) or match (`title`, `body`) against (? in natural language mode)"
    );
    let query = pgsql().table("posts").or_where_full_text_with(
        "search",
        "laravel",
        FullTextOptions {
            vector: true,
            language: Some("simple".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        query.to_sql(),
        "select * from \"posts\" where (\"search\") @@ plainto_tsquery('simple', ?)"
    );
    let query = pgsql().table("posts").where_full_text_with(
        "body",
        "laravel",
        FullTextOptions {
            mode: Some("websearch".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        query.to_sql(),
        "select * from \"posts\" where (to_tsvector('english', \"body\")) @@ websearch_to_tsquery('english', ?)"
    );
}

#[test]
fn json_clauses_with_or_variants() {
    let query = mysql()
        .table("users")
        .where_json_overlaps("options->languages", json!(["en", "fr"]))
        .or_where_json_doesnt_overlap("tags", json!(["php"]));
    assert_eq!(
        query.to_sql(),
        "select * from `users` where json_overlaps(json_extract(`options`, '$.\"languages\"'), ?) or not json_overlaps(`tags`, ?)"
    );
    assert_eq!(
        query.get_bindings(),
        vec![json!("[\"en\",\"fr\"]"), json!("[\"php\"]")]
    );
    assert_eq!(
        error_of(
            &pgsql()
                .table("users")
                .where_json_overlaps("tags", json!(["a"]))
        ),
        "This database engine does not support JSON overlaps operations."
    );

    let query = sqlite()
        .table("users")
        .where_("id", 1)
        .or_where_json_contains_key("options->theme")
        .or_where_json_doesnt_contain_key("options->languages")
        .or_where_json_length("options->languages", 2)
        .or_where_json_length_op("options->languages", ">", 3);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" = ? or json_type(\"options\", '$.\"theme\"') is not null or not json_type(\"options\", '$.\"languages\"') is not null or json_array_length(\"options\", '$.\"languages\"') = ? or json_array_length(\"options\", '$.\"languages\"') > ?"
    );
}

#[test]
fn binary_and_null_safe_comparisons() {
    let query = mysql()
        .table("users")
        .where_binary("name", "Taylor")
        .or_where_not_binary("name", "taylor");
    assert_eq!(
        query.to_sql(),
        "select * from `users` where `name` = binary ? or `name` != binary ?"
    );
    let query = mariadb().table("users").where_not_binary("name", "x");
    assert_eq!(
        query.to_sql(),
        "select * from `users` where `name` != binary ?"
    );
    assert_eq!(
        error_of(&sqlite().table("users").where_binary("name", "Taylor")),
        "This database engine does not support binary comparison operations."
    );

    let null_safe = |connection: Connection| {
        connection
            .table("users")
            .where_null_safe_equals("deleted_at", Value::Null)
            .or_where_null_safe_equals("name", "Taylor")
            .to_sql()
    };
    assert_eq!(
        null_safe(sqlite()),
        "select * from \"users\" where \"deleted_at\" is ? or \"name\" is ?"
    );
    assert_eq!(
        null_safe(mysql()),
        "select * from `users` where `deleted_at` <=> ? or `name` <=> ?"
    );
    assert_eq!(
        null_safe(pgsql()),
        "select * from \"users\" where \"deleted_at\" is not distinct from ? or \"name\" is not distinct from ?"
    );
}

#[test]
fn row_values_and_value_between_columns() {
    let query = sqlite()
        .table("orders")
        .where_row_values(["last_update", "order_number"], "<", (1, 2))
        .or_where_row_values(["a", "b"], "=", ["x", "y"]);
    assert_eq!(
        query.to_sql(),
        "select * from \"orders\" where (\"last_update\", \"order_number\") < (?, ?) or (\"a\", \"b\") = (?, ?)"
    );
    assert_eq!(
        query.get_bindings(),
        vec![json!(1), json!(2), json!("x"), json!("y")]
    );

    let error = sqlite()
        .table("orders")
        .where_row_values(["a", "b"], "<", (1,))
        .try_to_sql()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The number of columns must match the number of values"
    );
    assert!(error.downcast_ref::<InvalidArgumentException>().is_some());
    assert_eq!(
        error_of(
            &sqlite()
                .table("orders")
                .where_row_values(["a"], "nope", (1,))
        ),
        "Invalid operator passed to whereRowValues method."
    );

    let query = mysql()
        .table("products")
        .where_value_between(100, ["min_price", "max_price"])
        .or_where_value_not_between(5, ["min_qty", "max_qty"]);
    assert_eq!(
        query.to_sql(),
        "select * from `products` where ? between `min_price` and `max_price` or ? not between `min_qty` and `max_qty`"
    );
    assert_eq!(query.get_bindings(), vec![json!(100), json!(5)]);
    assert_eq!(
        sqlite()
            .table("p")
            .where_value_not_between(1, ["a", "b"])
            .or_where_value_between(raw("2"), ["c", "d"])
            .to_sql(),
        "select * from \"p\" where ? not between \"a\" and \"b\" or 2 between \"c\" and \"d\""
    );
}

// ----------------------------------------------------------------------
// Havings, orders and limits
// ----------------------------------------------------------------------

#[test]
fn having_or_variants_and_nested_havings() {
    let query = sqlite()
        .table("users")
        .group_by("name")
        .having_op("votes", ">", 1)
        .or_having_null("email")
        .or_having_not_null(["a", "b"])
        .or_having_not_between("votes", [1, 5])
        .or_having_nested(|query| query.having("x", 1).or_having("y", 2));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" group by \"name\" having \"votes\" > ? or \"email\" is null or \"a\" is not null or \"b\" is not null or \"votes\" not between ? and ? or (\"x\" = ? or \"y\" = ?)"
    );
    assert_eq!(
        query.get_bindings(),
        vec![json!(1), json!(1), json!(5), json!(1), json!(2)]
    );
    let query = sqlite()
        .table("users")
        .having_nested(|query| query.having("x", 1));
    assert_eq!(query.to_sql(), "select * from \"users\" having (\"x\" = ?)");
}

#[test]
fn in_order_of_and_reorder_desc() {
    let query = mysql()
        .table("posts")
        .order_by("id", "asc")
        .reorder_desc("created_at")
        .in_order_of("status", ["published", "draft"]);
    assert_eq!(
        query.to_sql(),
        "select * from `posts` order by `created_at` desc, case when `status` = ? then 0 when `status` = ? then 1 else 2 end"
    );
    assert_eq!(
        query.get_bindings(),
        vec![json!("published"), json!("draft")]
    );
    let empty: Vec<i64> = Vec::new();
    assert_eq!(
        sqlite().table("posts").in_order_of("id", empty).to_sql(),
        "select * from \"posts\""
    );
}

#[test]
fn group_limits_use_row_numbers() {
    let query = sqlite()
        .table("posts")
        .where_in("user_id", [1, 2])
        .order_by("created_at", "desc")
        .group_limit(3, "user_id");
    assert_eq!(
        query.to_sql(),
        "select * from (select *, row_number() over (partition by \"user_id\" order by \"created_at\" desc) as \"laravel_row\" from \"posts\" where \"user_id\" in (?, ?)) as \"laravel_table\" where \"laravel_row\" <= 3 order by \"laravel_row\""
    );

    let query = mysql()
        .table("posts")
        .select(["id", "user_id"])
        .order_by_raw("field(id, ?)", (5,))
        .offset(2)
        .group_limit(3, "posts.user_id")
        .where_("published", true);
    assert_eq!(
        query.to_sql(),
        "select * from (select `id`, `user_id`, row_number() over (partition by `posts`.`user_id` order by field(id, ?)) as `laravel_row` from `posts` where `published` = ?) as `laravel_table` where `laravel_row` <= 5 and `laravel_row` > 2 order by `laravel_row`"
    );
    // The order bindings move into the select clause.
    assert_eq!(query.get_bindings(), vec![json!(5), json!(true)]);
}

#[tokio::test]
async fn group_limits_limit_rows_per_group() {
    let db = seeded().await;
    let rows = db
        .table("users")
        .order_by("id", "asc")
        .group_limit(1, "votes")
        .get()
        .await
        .unwrap();
    // One user per distinct vote count (10, 5, 20, 1); `laravel_row` is removed.
    assert_eq!(rows.count(), 4);
    assert!(rows.iter().all(|row| row.get("laravel_row").is_none()));
    let names: Vec<Value> = rows.iter().map(|row| row["name"].clone()).collect();
    assert!(names.contains(&json!("Abigail")));
    assert!(!names.contains(&json!("James")));
}

// ----------------------------------------------------------------------
// Joins and index hints
// ----------------------------------------------------------------------

#[test]
fn lateral_joins() {
    let latest = |connection: &Connection| {
        connection
            .table("posts")
            .select(["id", "title"])
            .where_column("user_id", "users.id")
            .where_("published", true)
            .order_by_desc("created_at")
            .limit(3)
    };
    let db = pgsql();
    let query = db
        .table("users")
        .join_lateral(latest(&db), "latest_posts")
        .left_join_lateral(latest(&db), "other");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" inner join lateral (select \"id\", \"title\" from \"posts\" where \"user_id\" = \"users\".\"id\" and \"published\" = ? order by \"created_at\" desc limit 3) as \"latest_posts\" on true left join lateral (select \"id\", \"title\" from \"posts\" where \"user_id\" = \"users\".\"id\" and \"published\" = ? order by \"created_at\" desc limit 3) as \"other\" on true"
    );
    assert_eq!(query.get_bindings(), vec![json!(true), json!(true)]);

    let db = mysql();
    assert_eq!(
        db.table("users")
            .join_lateral(
                |query: Builder| query.from("posts").where_column("user_id", "users.id"),
                "p"
            )
            .to_sql(),
        "select * from `users` inner join lateral (select * from `posts` where `user_id` = `users`.`id`) as `p` on true"
    );
    for connection in [sqlite(), mariadb()] {
        let query = connection.table("users").join_lateral("select 1", "p");
        assert_eq!(
            error_of(&query),
            "This database engine does not support lateral joins."
        );
    }
}

#[test]
fn straight_joins() {
    let db = mysql();
    let query = db
        .table("users")
        .straight_join("contacts", "users.id", "=", "contacts.user_id")
        .straight_join_where("photos", "photos.user_id", "=", 5)
        .straight_join_sub(db.table("posts"), "p", "p.user_id", "=", "users.id");
    assert_eq!(
        query.to_sql(),
        "select * from `users` straight_join `contacts` on `users`.`id` = `contacts`.`user_id` straight_join `photos` on `photos`.`user_id` = ? straight_join (select * from `posts`) as `p` on `p`.`user_id` = `users`.`id`"
    );
    let query =
        sqlite()
            .table("users")
            .straight_join("contacts", "users.id", "=", "contacts.user_id");
    assert_eq!(
        error_of(&query),
        "This database engine does not support straight joins."
    );
}

#[test]
fn index_hints() {
    assert_eq!(
        mysql().table("users").use_index("test_index").to_sql(),
        "select * from `users` use index (test_index)"
    );
    assert_eq!(
        mysql()
            .table("users")
            .force_index("a, b")
            .where_("id", 1)
            .to_sql(),
        "select * from `users` force index (a, b) where `id` = ?"
    );
    assert_eq!(
        mariadb().table("users").ignore_index("test_index").to_sql(),
        "select * from `users` ignore index (test_index)"
    );
    assert_eq!(
        sqlite().table("users").force_index("test_index").to_sql(),
        "select * from \"users\" indexed by test_index"
    );
    assert_eq!(
        sqlite().table("users").use_index("test_index").to_sql(),
        "select * from \"users\""
    );
    let error = mysql()
        .table("users")
        .use_index("test_index; drop table users")
        .try_to_sql()
        .unwrap_err();
    assert_eq!(error.to_string(), "Index name contains invalid characters.");
}

// ----------------------------------------------------------------------
// Selects, timeouts and callbacks
// ----------------------------------------------------------------------

#[test]
fn select_expressions_and_timeouts() {
    assert_eq!(
        mysql()
            .table("users")
            .select_expression(raw("votes * 2"), "double")
            .to_sql(),
        "select (votes * 2) as `double` from `users`"
    );
    assert_eq!(
        mysql().table("users").timeout(5).to_sql(),
        "select /*+ MAX_EXECUTION_TIME(5000) */ * from `users`"
    );
    assert_eq!(
        sqlite().table("users").timeout(5).to_sql(),
        "select * from \"users\""
    );
    assert_eq!(
        mysql().table("users").timeout(5).timeout(None).to_sql(),
        "select * from `users`"
    );
    assert_eq!(
        error_of(&sqlite().table("users").timeout(0)),
        "Timeout must be greater than zero."
    );
}

#[tokio::test]
async fn before_and_after_query_callbacks() {
    let db = seeded().await;
    let query = db
        .table("users")
        .before_query(|query| query.where_op("votes", ">", 5))
        .order_by("id", "asc");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"votes\" > ? order by \"id\" asc"
    );
    assert_eq!(query.get().await.unwrap().count(), 2);
    assert_eq!(query.count().await.unwrap(), 2);
    assert!(
        query
            .clone()
            .where_("name", "Taylor")
            .exists()
            .await
            .unwrap()
    );
    assert_eq!(query.clone().apply_before_query_callbacks().wheres.len(), 1);

    let names = db
        .table("users")
        .order_by("id", "asc")
        .after_query(|users: Collection<Value>| {
            users
                .into_iter()
                .map(|mut user| {
                    user["name"] = json!(user["name"].as_str().unwrap().to_uppercase());
                    user
                })
                .collect()
        })
        .pluck("name")
        .await
        .unwrap();
    assert_eq!(names[0], json!("TAYLOR"));

    // Aggregates aren't rows: the callbacks don't apply.
    let count = db
        .table("users")
        .after_query(|_| Collection::new())
        .count()
        .await
        .unwrap();
    assert_eq!(count, 5);
}

// ----------------------------------------------------------------------
// Retrieval helpers
// ----------------------------------------------------------------------

#[tokio::test]
async fn or_callbacks_and_raw_values() {
    let db = seeded().await;
    let users = db.table("users");

    let found = users
        .find_or(1, || async {
            Ok::<_, illuminate_support::Error>(json!(null))
        })
        .await
        .unwrap();
    assert_eq!(found["name"], json!("Taylor"));
    let fallback = users
        .find_or(99, || async {
            Ok::<_, illuminate_support::Error>(json!({"name": "Guest"}))
        })
        .await
        .unwrap();
    assert_eq!(fallback["name"], json!("Guest"));
    let error = users
        .clone()
        .where_("name", "Nobody")
        .first_or(|| async { Err(RuntimeException::new("Missing!")) })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Missing!");

    assert!(
        users
            .clone()
            .where_("id", 1)
            .exists_or(|| async { Ok::<_, RuntimeException>(false) })
            .await
            .unwrap()
    );
    assert!(
        !users
            .clone()
            .where_("id", 99)
            .exists_or(|| async { Ok::<_, RuntimeException>(false) })
            .await
            .unwrap()
    );
    assert!(
        users
            .clone()
            .where_("id", 99)
            .doesnt_exist_or(|| async { Err::<bool, _>(RuntimeException::new("exists")) })
            .await
            .unwrap()
    );
    assert!(
        users
            .clone()
            .where_("id", 1)
            .doesnt_exist_or(|| async { Err::<bool, _>(RuntimeException::new("exists")) })
            .await
            .is_err()
    );

    assert_eq!(
        users.raw_value("max(votes)", ()).await.unwrap(),
        Some(json!(20))
    );
    assert_eq!(
        users
            .clone()
            .where_("votes", 5)
            .raw_value("count(*) + ?", (10,))
            .await
            .unwrap(),
        Some(json!(12))
    );
}

#[tokio::test]
async fn live_where_clauses_on_sqlite() {
    let db = seeded().await;
    let users = db.table("users");
    let found = users
        .clone()
        .where_value_between(50, ["min_votes", "max_votes"])
        .count()
        .await
        .unwrap();
    assert_eq!(found, 5);
    let found = users
        .clone()
        .where_row_values(["votes", "id"], ">", (5, 2))
        .pluck("name")
        .await
        .unwrap();
    assert_eq!(
        found.all(),
        &[json!("Taylor"), json!("James"), json!("Dayle")]
    );
    let found = users
        .clone()
        .where_null_safe_equals("options", Value::Null)
        .count()
        .await
        .unwrap();
    assert_eq!(found, 3);
    let found = users
        .clone()
        .where_("name", "Jess")
        .or_where_json_contains_key("options->theme")
        .count()
        .await
        .unwrap();
    assert_eq!(found, 2);
    let found = users
        .clone()
        .in_order_of("name", ["Jess", "Taylor"])
        .order_by("id", "asc")
        .pluck("name")
        .await
        .unwrap();
    assert_eq!(found[0], json!("Jess"));
    assert_eq!(found[1], json!("Taylor"));
    assert_eq!(found[2], json!("Abigail"));
}

#[tokio::test]
async fn insert_or_ignore_returning() {
    let db = seeded().await;
    let inserted = db
        .table("users")
        .insert_or_ignore_returning(
            json!([
                {"name": "Taylor", "email": "taylor@laravel.com"},
                {"name": "Nuno", "email": "nuno@laravel.com"},
            ]),
            &["id", "email"],
            Some(&["email"]),
        )
        .await
        .unwrap();
    assert_eq!(inserted.count(), 1);
    assert_eq!(inserted[0]["email"], json!("nuno@laravel.com"));
    assert!(inserted[0]["id"].as_i64().unwrap() > 5);

    let none = db
        .table("users")
        .insert_or_ignore_returning(
            json!({"name": "Taylor", "email": "taylor@laravel.com"}),
            &["*"],
            None,
        )
        .await
        .unwrap();
    assert!(none.is_empty());

    let error = db
        .table("users")
        .insert_or_ignore_returning(json!({"name": "x"}), &[], None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The returning columns must not be empty."
    );
    let error = db
        .table("users")
        .insert_or_ignore_returning(json!({"name": "x"}), &["id"], Some(&[]))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "The unique columns must not be empty.");

    let error = mysql()
        .table("users")
        .insert_or_ignore_returning(json!({"name": "x"}), &["id"], None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "This database engine does not support insert or ignore with returning."
    );
    let records = vec![vec![(
        "email".to_string(),
        illuminate_database::Operand::from("a@b.c"),
    )]];
    let db = pgsql();
    let query = db.table("users");
    assert_eq!(
        query
            .get_grammar()
            .compile_insert_or_ignore_returning(
                &query,
                &records,
                &["id".into()],
                Some(&["email".to_string()])
            )
            .unwrap(),
        "insert into \"users\" (\"email\") values (?) on conflict (\"email\") do nothing returning \"id\""
    );
}

#[tokio::test]
async fn update_from_compiles_joins_into_from_clauses() {
    let db = pgsql();
    let query = db
        .table("users")
        .join_with("orders", |join| {
            join.on("users.id", "=", "orders.user_id")
                .where_("orders.status", "paid")
        })
        .where_("users.active", true);
    let record = vec![(
        "users.balance".to_string(),
        illuminate_database::Operand::from(Expression::new("orders.total")),
    )];
    let grammar = query.get_grammar();
    assert_eq!(
        grammar.compile_update_from(&query, &record).unwrap(),
        "update \"users\" set \"balance\" = orders.total from \"orders\" where \"users\".\"active\" = ? and \"users\".\"id\" = \"orders\".\"user_id\" and \"orders\".\"status\" = ?"
    );
    assert_eq!(
        grammar.prepare_bindings_for_update_from(query.get_raw_bindings(), &record),
        vec![json!(true), json!("paid")]
    );
    let query = db
        .table("users")
        .join("orders", "users.id", "=", "orders.user_id");
    assert_eq!(
        query
            .get_grammar()
            .compile_update_from(&query, &record)
            .unwrap(),
        "update \"users\" set \"balance\" = orders.total from \"orders\" where \"users\".\"id\" = \"orders\".\"user_id\""
    );

    let error = sqlite()
        .table("users")
        .update_from(json!({"votes": 1}))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "This database engine does not support the updateFrom method."
    );
    assert!(error.downcast_ref::<LogicException>().is_some());
}

// ----------------------------------------------------------------------
// Cursors and pagination
// ----------------------------------------------------------------------

#[tokio::test]
async fn cursors_stream_rows() {
    let db = seeded().await;
    let names: Vec<Value> = db
        .table("users")
        .order_by("id", "asc")
        .after_query(|users: Collection<Value>| {
            users
                .into_iter()
                .filter(|user| user["votes"] != json!(5))
                .collect()
        })
        .cursor()
        .map(|row| row.unwrap()["name"].clone())
        .collect()
        .await;
    assert_eq!(names, vec![json!("Taylor"), json!("Dayle"), json!("Jess")]);

    let mut failing = db.table("missing").cursor();
    assert!(failing.next().await.unwrap().is_err());
}

#[tokio::test]
async fn cursors_stream_from_a_pooled_connection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cursor.sqlite");
    let db = Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": path.to_string_lossy()}),
    );
    db.get_schema_builder()
        .create("numbers", |table| {
            table.id();
            table.integer("value");
        })
        .await
        .unwrap();
    let rows: Vec<Value> = (1..=150).map(|i| json!({"value": i})).collect();
    db.table("numbers")
        .insert(Value::Array(rows))
        .await
        .unwrap();

    db.enable_query_log();
    let mut stream = db.table("numbers").order_by("id", "asc").cursor();
    let mut total = 0;
    let mut seen = 0;
    while let Some(row) = stream.next().await {
        let row = row.unwrap();
        total += row["value"].as_i64().unwrap();
        seen += 1;
        if seen == 10 {
            // Other queries keep working while the cursor is open.
            assert_eq!(db.table("numbers").count().await.unwrap(), 150);
        }
    }
    assert_eq!(seen, 150);
    assert_eq!(total, (1..=150).sum::<i64>());
    let log = db.get_query_log();
    assert!(
        log.iter()
            .any(|entry| entry.query == "select * from \"numbers\" order by \"id\" asc")
    );
}

#[tokio::test]
async fn length_aware_and_simple_pagination() {
    let db = seeded().await;
    let page = db
        .table("users")
        .order_by("id", "asc")
        .paginate_with(2, "page", Some(2))
        .await
        .unwrap();
    assert_eq!(page.total(), 5);
    assert_eq!(page.items()[0]["name"], json!("James"));
    let page = db
        .table("users")
        .order_by("id", "asc")
        .simple_paginate_with(2, "page", Some(3))
        .await
        .unwrap();
    assert_eq!(page.count(), 1);
    assert!(!page.has_more_pages());
    let page = db.table("users").paginate(10).await.unwrap();
    assert_eq!(page.count(), 5);
    let page = db.table("users").simple_paginate(10).await.unwrap();
    assert_eq!(page.count(), 5);
}

#[test]
fn cursor_pagination_constraints() {
    let db = sqlite();
    let cursor = Cursor::new(json!({"votes": 5, "id": 3}), true);
    let (query, parameters) = db
        .table("users")
        .order_by("votes", "desc")
        .order_by("id", "asc")
        .prepare_cursor_pagination(10, Some(&cursor))
        .unwrap();
    assert_eq!(parameters, vec!["votes", "id"]);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where (\"votes\" < ? or (\"votes\" = ? and (\"id\" > ?))) order by \"votes\" desc, \"id\" asc limit 11"
    );
    assert_eq!(query.get_bindings(), vec![json!(5), json!(5), json!(3)]);

    // Going back reverses the orderings.
    let back = Cursor::new(json!({"id": 3}), false);
    let (query, _) = db
        .table("users")
        .order_by("id", "asc")
        .prepare_cursor_pagination(2, Some(&back))
        .unwrap();
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where (\"id\" < ?) order by \"id\" desc limit 3"
    );

    // Aliased expressions are compared by their original expression.
    let (query, _) = db
        .table("users")
        .select(vec![
            illuminate_database::Ident::from("id"),
            raw("abs(votes) as score").into(),
        ])
        .order_by("score", "desc")
        .prepare_cursor_pagination(2, Some(&Cursor::new(json!({"score": 10}), true)))
        .unwrap();
    assert_eq!(
        query.to_sql(),
        "select \"id\", abs(votes) as score from \"users\" where (abs(votes) < ?) order by \"score\" desc limit 3"
    );
}

#[tokio::test]
async fn cursor_pagination_walks_forwards_and_backwards() {
    let db = seeded().await;
    let query = db
        .table("users")
        .order_by("votes", "desc")
        .order_by("id", "asc");

    let first: CursorPaginator<Value> = query.cursor_paginate(2, None).await.unwrap();
    let names = |page: &CursorPaginator<Value>| -> Vec<Value> {
        page.items()
            .iter()
            .map(|user| user["name"].clone())
            .collect()
    };
    assert_eq!(names(&first), vec![json!("Dayle"), json!("Taylor")]);
    assert!(first.on_first_page());
    assert!(first.previous_cursor().is_none());
    let next = first.next_cursor().unwrap();
    assert_eq!(next.parameter("votes").unwrap(), json!(10));

    let second = query.cursor_paginate(2, Some(next)).await.unwrap();
    assert_eq!(names(&second), vec![json!("Abigail"), json!("James")]);
    assert!(!second.on_first_page());
    let third = query
        .cursor_paginate(2, second.next_cursor())
        .await
        .unwrap();
    assert_eq!(names(&third), vec![json!("Jess")]);
    assert!(third.next_cursor().is_none());
    assert!(third.on_last_page());

    let back = query
        .cursor_paginate(2, third.previous_cursor())
        .await
        .unwrap();
    assert_eq!(names(&back), vec![json!("Abigail"), json!("James")]);
    let start = query
        .cursor_paginate(2, back.previous_cursor())
        .await
        .unwrap();
    assert_eq!(names(&start), vec![json!("Dayle"), json!("Taylor")]);
    assert!(start.on_first_page());

    let json = third.to_array();
    assert_eq!(json["next_cursor"], Value::Null);
    assert!(json["prev_page_url"].as_str().unwrap().contains("cursor="));

    let error = db
        .table("users")
        .cursor_paginate(2, None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "You must specify an orderBy clause when using this function."
    );
    let error = query
        .cursor_paginate(2, Some(Cursor::new(json!({"id": 1}), true)))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Unable to find parameter [votes] in pagination item."
    );
}

#[tokio::test]
async fn the_current_cursor_comes_from_the_resolver() {
    let db = seeded().await;
    let cursor = Cursor::new(json!({"id": 4}), true).encode();
    illuminate_database::pagination::resolve_current_cursor_using(move |name| {
        (name == "users_cursor").then(|| cursor.clone())
    });
    let page = db
        .table("users")
        .order_by("id", "asc")
        .cursor_paginate_with(2, "users_cursor", None)
        .await
        .unwrap();
    assert_eq!(page.items()[0]["name"], json!("Jess"));
    assert_eq!(page.get_cursor_name(), "users_cursor");
    illuminate_database::pagination::resolve_current_cursor_using(|_| None);
}
