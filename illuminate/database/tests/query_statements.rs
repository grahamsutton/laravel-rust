//! Compiling insert / update / delete / upsert / aggregate statements, for all
//! three grammars. MySQL and PostgreSQL statements are checked through
//! `pretend`, which runs the whole pipeline without a server.

use illuminate_database::query::{BindingType, UpsertColumn};
use illuminate_database::{
    Builder, Connection, IntoRecord, IntoRecords, JoinClause, QueryLog, raw,
};
use illuminate_support::{Result, json};

fn sqlite() -> Connection {
    Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:"}),
    )
}

fn mysql() -> Connection {
    Connection::new("mysql", json!({"driver": "mysql", "database": "laravel"}))
}

fn pgsql() -> Connection {
    Connection::new("pgsql", json!({"driver": "pgsql", "database": "laravel"}))
}

async fn pretend<F, Fut>(connection: &Connection, callback: F) -> Vec<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    connection
        .pretend(callback)
        .await
        .unwrap()
        .into_iter()
        .map(|log: QueryLog| log.query)
        .collect()
}

#[test]
fn inserts() {
    for (connection, expected) in [
        (sqlite(), "insert into \"users\" (\"email\") values (?)"),
        (mysql(), "insert into `users` (`email`) values (?)"),
        (pgsql(), "insert into \"users\" (\"email\") values (?)"),
    ] {
        let query = connection.table("users");
        let records = json!({"email": "foo"}).into_records();
        assert_eq!(
            query.get_grammar().compile_insert(&query, &records),
            expected
        );
    }

    let query = sqlite().table("users");
    let records = json!([{"email": "foo", "name": "taylor"}, {"email": "bar", "name": "dayle"}])
        .into_records();
    assert_eq!(
        query.get_grammar().compile_insert(&query, &records),
        "insert into \"users\" (\"email\", \"name\") values (?, ?), (?, ?)"
    );

    assert_eq!(
        query.get_grammar().compile_insert(&query, &[]),
        "insert into \"users\" default values"
    );
    let query = mysql().table("users");
    assert_eq!(
        query.get_grammar().compile_insert(&query, &[]),
        "insert into `users` () values ()"
    );
}

#[test]
fn insert_or_ignore_and_get_id() {
    let records = json!({"email": "foo"}).into_records();
    let query = sqlite().table("users");
    assert_eq!(
        query
            .get_grammar()
            .compile_insert_or_ignore(&query, &records),
        "insert or ignore into \"users\" (\"email\") values (?)"
    );
    let query = mysql().table("users");
    assert_eq!(
        query
            .get_grammar()
            .compile_insert_or_ignore(&query, &records),
        "insert ignore into `users` (`email`) values (?)"
    );
    let query = pgsql().table("users");
    assert_eq!(
        query
            .get_grammar()
            .compile_insert_or_ignore(&query, &records),
        "insert into \"users\" (\"email\") values (?) on conflict do nothing"
    );
    assert_eq!(
        query
            .get_grammar()
            .compile_insert_get_id(&query, &records, "id"),
        "insert into \"users\" (\"email\") values (?) returning \"id\""
    );
    let query = sqlite().table("users");
    assert_eq!(
        query
            .get_grammar()
            .compile_insert_get_id(&query, &records, "id"),
        "insert into \"users\" (\"email\") values (?)"
    );
}

#[test]
fn insert_using() {
    let query = sqlite().table("table1");
    assert_eq!(
        query.get_grammar().compile_insert_using(
            &query,
            &["foo".into()],
            "select \"bar\" from \"table2\""
        ),
        "insert into \"table1\" (\"foo\") select \"bar\" from \"table2\""
    );
    let query = mysql().table("table1");
    assert_eq!(
        query
            .get_grammar()
            .compile_insert_or_ignore_using(&query, &[], "select * from `table2`"),
        "insert ignore into `table1` select * from `table2`"
    );
}

#[test]
fn upserts() {
    let records = json!([
        {"email": "foo", "name": "bar"},
        {"name": "bar2", "email": "foo2"},
    ]);
    let mut records = records.into_records();
    for record in records.iter_mut() {
        record.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let update = vec![
        UpsertColumn::Column("email".into()),
        UpsertColumn::Column("name".into()),
    ];

    let query = mysql().table("users");
    assert_eq!(
        query
            .get_grammar()
            .compile_upsert(&query, &records, &["email".into()], &update),
        "insert into `users` (`email`, `name`) values (?, ?), (?, ?) on duplicate key update `email` = values(`email`), `name` = values(`name`)"
    );

    let aliased = Connection::new(
        "mysql",
        json!({"driver": "mysql", "use_upsert_alias": true}),
    );
    let query = aliased.table("users");
    assert_eq!(
        query
            .get_grammar()
            .compile_upsert(&query, &records, &["email".into()], &update),
        "insert into `users` (`email`, `name`) values (?, ?), (?, ?) as laravel_upsert_alias on duplicate key update `email` = `laravel_upsert_alias`.`email`, `name` = `laravel_upsert_alias`.`name`"
    );

    for connection in [pgsql(), sqlite()] {
        let query = connection.table("users");
        assert_eq!(
            query
                .get_grammar()
                .compile_upsert(&query, &records, &["email".into()], &update),
            "insert into \"users\" (\"email\", \"name\") values (?, ?), (?, ?) on conflict (\"email\") do update set \"email\" = \"excluded\".\"email\", \"name\" = \"excluded\".\"name\""
        );
    }
}

#[test]
fn updates() {
    let values = json!({"email": "foo", "name": "bar"}).into_record();

    let query = sqlite().table("users").where_("id", 1);
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update \"users\" set \"email\" = ?, \"name\" = ? where \"id\" = ?"
    );
    assert_eq!(
        query
            .get_grammar()
            .prepare_bindings_for_update(&query.bindings, &values),
        vec![json!("foo"), json!("bar"), json!(1)]
    );

    let query = mysql()
        .table("users")
        .where_("id", 1)
        .order_by("foo", "desc")
        .limit(5);
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update `users` set `email` = ?, `name` = ? where `id` = ? order by `foo` desc limit 5"
    );

    let query = mysql()
        .table("users")
        .join("orders", "users.id", "=", "orders.user_id")
        .where_("users.id", 1);
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update `users` inner join `orders` on `users`.`id` = `orders`.`user_id` set `email` = ?, `name` = ? where `users`.`id` = ?"
    );

    let query = sqlite()
        .table("users")
        .join("orders", "users.id", "=", "orders.user_id")
        .where_("users.id", 1);
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update \"users\" set \"email\" = ?, \"name\" = ? where \"rowid\" in (select \"users\".\"rowid\" from \"users\" inner join \"orders\" on \"users\".\"id\" = \"orders\".\"user_id\" where \"users\".\"id\" = ?)"
    );

    let query = pgsql().table("users").where_("id", 1).limit(1);
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update \"users\" set \"email\" = ?, \"name\" = ? where \"ctid\" in (select \"users\".\"ctid\" from \"users\" where \"id\" = ? limit 1)"
    );

    let query = sqlite().table("users as u").where_("u.id", 1).limit(1);
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update \"users\" as \"u\" set \"email\" = ?, \"name\" = ? where \"rowid\" in (select \"u\".\"rowid\" from \"users\" as \"u\" where \"u\".\"id\" = ? limit 1)"
    );
}

#[test]
fn json_updates() {
    let values = json!({"options->name": "Taylor", "options->enabled": true, "meta->tags": ["a"]})
        .into_record();

    let query = mysql().table("users");
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update `users` set `options` = json_set(`options`, '$.\"name\"', ?), `options` = json_set(`options`, '$.\"enabled\"', true), `meta` = json_set(`meta`, '$.\"tags\"', cast(? as json))"
    );
    assert_eq!(
        query
            .get_grammar()
            .prepare_bindings_for_update(&query.bindings, &values),
        vec![json!("Taylor"), json!("[\"a\"]")]
    );

    let values = json!({"options->name->first": "Taylor"}).into_record();
    let query = pgsql().table("users");
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update \"users\" set \"options\" = jsonb_set(\"options\"::jsonb, '{\"name\",\"first\"}', ?)"
    );
    assert_eq!(
        query
            .get_grammar()
            .prepare_bindings_for_update(&query.bindings, &values),
        vec![json!("\"Taylor\"")]
    );

    let values = json!({"name": "x", "options->a": 1, "options->b->c": 2}).into_record();
    let query = sqlite().table("users");
    assert_eq!(
        query.get_grammar().compile_update(&query, &values).unwrap(),
        "update \"users\" set \"name\" = ?, \"options\" = json_patch(ifnull(\"options\", json('{}')), json(?))"
    );
    assert_eq!(
        query
            .get_grammar()
            .prepare_bindings_for_update(&query.bindings, &values),
        vec![json!("x"), json!("{\"a\":1,\"b\":{\"c\":2}}")]
    );
}

#[test]
fn deletes() {
    let query = sqlite().table("users").where_("email", "foo");
    assert_eq!(
        query.get_grammar().compile_delete(&query).unwrap(),
        "delete from \"users\" where \"email\" = ?"
    );

    let query = sqlite()
        .table("users")
        .where_("email", "foo")
        .order_by("id", "asc")
        .limit(1);
    assert_eq!(
        query.get_grammar().compile_delete(&query).unwrap(),
        "delete from \"users\" where \"rowid\" in (select \"users\".\"rowid\" from \"users\" where \"email\" = ? order by \"id\" asc limit 1)"
    );

    let query = mysql()
        .table("users")
        .where_("email", "foo")
        .order_by("id", "asc")
        .limit(1);
    assert_eq!(
        query.get_grammar().compile_delete(&query).unwrap(),
        "delete from `users` where `email` = ? order by `id` asc limit 1"
    );

    let query = mysql()
        .table("users as a")
        .join_with("users as b", |j: JoinClause| j.on("a.id", "=", "b.user_id"))
        .where_("email", "foo");
    assert_eq!(
        query.get_grammar().compile_delete(&query).unwrap(),
        "delete `a` from `users` as `a` inner join `users` as `b` on `a`.`id` = `b`.`user_id` where `email` = ?"
    );

    let query = pgsql()
        .table("users")
        .join("contacts", "users.id", "=", "contacts.id")
        .where_("email", "foo");
    assert_eq!(
        query.get_grammar().compile_delete(&query).unwrap(),
        "delete from \"users\" where \"ctid\" in (select \"users\".\"ctid\" from \"users\" inner join \"contacts\" on \"users\".\"id\" = \"contacts\".\"id\" where \"email\" = ?)"
    );
}

#[test]
fn truncates() {
    let query = sqlite().table("users");
    assert_eq!(
        query.get_grammar().compile_truncate(&query),
        vec![
            (
                "delete from sqlite_sequence where name = ?".to_string(),
                vec![json!("users")]
            ),
            ("delete from \"users\"".to_string(), vec![]),
        ]
    );
    let query = mysql().table("users");
    assert_eq!(
        query.get_grammar().compile_truncate(&query)[0].0,
        "truncate table `users`"
    );
    let query = pgsql().table("users");
    assert_eq!(
        query.get_grammar().compile_truncate(&query)[0].0,
        "truncate \"users\" restart identity cascade"
    );
}

#[test]
fn exists_compiles_per_grammar() {
    let query = sqlite().table("users").where_("id", 1);
    assert_eq!(
        query.get_grammar().compile_exists(&query).unwrap(),
        "select exists(select * from \"users\" where \"id\" = ?) as \"exists\""
    );
    let query = mysql().table("users");
    assert_eq!(
        query.get_grammar().compile_exists(&query).unwrap(),
        "select exists(select * from `users`) as `exists`"
    );
}

#[tokio::test]
async fn mysql_statements_through_pretend() {
    let db = mysql();
    let queries = pretend(&db, || async {
        db.table("users")
            .where_("id", 1)
            .update(json!({"name": "Taylor"}))
            .await?;
        db.table("users")
            .insert(json!({"name": "O'Brien", "admin": true}))
            .await?;
        db.table("users")
            .where_op("votes", ">", 100)
            .delete()
            .await?;
        db.table("users").where_("active", 1).count().await?;
        db.table("users").max("votes").await?;
        db.table("users").exists().await?;
        db.table("users").truncate().await?;
        db.table("users").increment("votes", 5).await?;
        db.table("users")
            .decrement_with("votes", 1, json!({"name": "x"}))
            .await?;
        Ok(())
    })
    .await;
    assert_eq!(
        queries,
        vec![
            "update `users` set `name` = 'Taylor' where `id` = 1",
            "insert into `users` (`name`, `admin`) values ('O''Brien', 1)",
            "delete from `users` where `votes` > 100",
            "select count(*) as `aggregate` from `users` where `active` = 1",
            "select max(`votes`) as `aggregate` from `users`",
            "select exists(select * from `users`) as `exists`",
            "truncate table `users`",
            "update `users` set `votes` = `votes` + 5",
            "update `users` set `votes` = `votes` - 1, `name` = 'x'",
        ]
    );
}

#[tokio::test]
async fn postgres_statements_through_pretend() {
    let db = pgsql();
    let queries = pretend(&db, || async {
        db.table("users")
            .insert_get_id(json!({"email": "taylor@laravel.com"}))
            .await?;
        db.table("users")
            .upsert(
                json!([{"email": "a", "votes": 1}, {"email": "b", "votes": 2}]),
                &["email"],
                Some(&["votes"]),
            )
            .await?;
        db.table("users")
            .where_in("id", [1, 2])
            .update(json!({"active": false}))
            .await?;
        Ok(())
    })
    .await;
    assert_eq!(
        queries,
        vec![
            "insert into \"users\" (\"email\") values ('taylor@laravel.com') returning \"id\"",
            "insert into \"users\" (\"email\", \"votes\") values ('a', 1), ('b', 2) on conflict (\"email\") do update set \"votes\" = \"excluded\".\"votes\"",
            "update \"users\" set \"active\" = false where \"id\" in (1, 2)",
        ]
    );
}

#[test]
fn aggregates_drop_columns_and_orders() {
    // `count` compiles the query without its columns, select bindings, and orders.
    let query = sqlite()
        .table("users")
        .select_raw("? as x", (1,))
        .order_by("name", "asc")
        .where_("a", 1);
    let count = query
        .clone_without(&["columns"])
        .clone_without_bindings(&[BindingType::Select]);
    assert_eq!(count.get_bindings(), vec![json!(1)]);
}

#[test]
fn update_bindings_with_joins_put_join_bindings_first_on_mysql() {
    let query = mysql()
        .table("users")
        .join_with("orders", |j: JoinClause| {
            j.on("users.id", "=", "orders.user_id")
                .where_("orders.paid", true)
        })
        .where_("users.id", 7);
    let values = json!({"name": "x"}).into_record();
    assert_eq!(
        query
            .get_grammar()
            .prepare_bindings_for_update(&query.bindings, &values),
        vec![json!(true), json!("x"), json!(7)]
    );
}

#[test]
fn increments_use_raw_expressions() {
    let builder: Builder = sqlite().table("users");
    let values = vec![("votes", raw("\"votes\" + 1"))].into_record();
    assert_eq!(
        builder
            .get_grammar()
            .compile_update(&builder, &values)
            .unwrap(),
        "update \"users\" set \"votes\" = \"votes\" + 1"
    );
}
