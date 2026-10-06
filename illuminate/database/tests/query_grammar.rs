//! SQL generation tests for the SQLite, MySQL / MariaDB and PostgreSQL query grammars.

use illuminate_database::query::{FullTextOptions, Lock};
use illuminate_database::{Builder, Connection, Expression, JoinClause, raw};
use illuminate_support::{Conditionable, Value, json};

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

fn users(connection: Connection) -> Builder {
    connection.table("users")
}

#[test]
fn basic_selects() {
    assert_eq!(users(sqlite()).to_sql(), "select * from \"users\"");
    assert_eq!(users(mysql()).to_sql(), "select * from `users`");
    assert_eq!(users(pgsql()).to_sql(), "select * from \"users\"");

    let query = users(sqlite()).select(["id", "name as n", "users.email"]);
    assert_eq!(
        query.to_sql(),
        "select \"id\", \"name\" as \"n\", \"users\".\"email\" from \"users\""
    );

    let query = users(mysql()).select(["id", "name as n", "users.email"]);
    assert_eq!(
        query.to_sql(),
        "select `id`, `name` as `n`, `users`.`email` from `users`"
    );
}

#[test]
fn add_select_skips_duplicates_and_raw_selects_bind() {
    let query = users(sqlite())
        .select("foo")
        .add_select("bar")
        .add_select(["baz", "boom"])
        .add_select("bar");
    assert_eq!(
        query.to_sql(),
        "select \"foo\", \"bar\", \"baz\", \"boom\" from \"users\""
    );

    let query = users(sqlite()).select_raw("substr(foo, 6, ?) as bar", (5,));
    assert_eq!(
        query.to_sql(),
        "select substr(foo, 6, 5) as bar from \"users\"".replace("5)", "?)")
    );
    assert_eq!(query.get_bindings(), vec![json!(5)]);

    let query = users(sqlite())
        .select(raw("count(*) as user_count"))
        .add_select("status");
    assert_eq!(
        query.to_sql(),
        "select count(*) as user_count, \"status\" from \"users\""
    );
}

#[test]
fn distinct_selects() {
    assert_eq!(
        users(sqlite()).distinct().select(["foo", "bar"]).to_sql(),
        "select distinct \"foo\", \"bar\" from \"users\""
    );
    assert_eq!(
        users(pgsql()).distinct_on(["age"]).select(["foo"]).to_sql(),
        "select distinct on (\"age\") \"foo\" from \"users\""
    );
    assert_eq!(
        users(mysql()).distinct_on(["age"]).select(["foo"]).to_sql(),
        "select distinct `foo` from `users`"
    );
}

#[test]
fn table_aliases_and_prefixes() {
    assert_eq!(
        sqlite().table("users as people").to_sql(),
        "select * from \"users\" as \"people\""
    );
    assert_eq!(
        sqlite().query().from_as("users", "u").to_sql(),
        "select * from \"users\" as \"u\""
    );

    let connection = Connection::new("sqlite", json!({"driver": "sqlite", "prefix": "prefix_"}));
    assert_eq!(
        connection.table("users").to_sql(),
        "select * from \"prefix_users\""
    );
    assert_eq!(
        connection
            .table("users as people")
            .select("people.name")
            .to_sql(),
        "select \"prefix_people\".\"name\" from \"prefix_users\" as \"prefix_people\""
    );
    assert_eq!(
        connection.table("public.users").to_sql(),
        "select * from \"public\".\"prefix_users\""
    );
}

#[test]
fn basic_wheres() {
    let query = users(sqlite()).where_("id", 1);
    assert_eq!(query.to_sql(), "select * from \"users\" where \"id\" = ?");
    assert_eq!(query.get_bindings(), vec![json!(1)]);

    let query = users(mysql())
        .where_op("votes", ">", 100)
        .or_where("name", "John");
    assert_eq!(
        query.to_sql(),
        "select * from `users` where `votes` > ? or `name` = ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(100), json!("John")]);

    let query = users(pgsql())
        .where_op("votes", "<>", 100)
        .or_where_op("votes", "<=", 5);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"votes\" <> ? or \"votes\" <= ?"
    );
}

#[test]
fn where_with_invalid_operator_treats_it_as_the_value() {
    let query = users(sqlite()).where_op("name", "taylor", "ignored");
    assert_eq!(query.to_sql(), "select * from \"users\" where \"name\" = ?");
    assert_eq!(query.get_bindings(), vec![json!("taylor")]);
}

#[test]
fn null_values_become_null_checks() {
    assert_eq!(
        users(sqlite()).where_("deleted_at", Value::Null).to_sql(),
        "select * from \"users\" where \"deleted_at\" is null"
    );
    assert_eq!(
        users(sqlite())
            .where_op("deleted_at", "!=", None::<i64>)
            .to_sql(),
        "select * from \"users\" where \"deleted_at\" is not null"
    );
    assert!(
        users(sqlite())
            .where_op("deleted_at", ">", Value::Null)
            .try_to_sql()
            .is_err()
    );
}

#[test]
fn where_null_and_not_null() {
    assert_eq!(
        users(sqlite()).where_null("id").to_sql(),
        "select * from \"users\" where \"id\" is null"
    );
    assert_eq!(
        users(sqlite()).where_null(["id", "expires_at"]).to_sql(),
        "select * from \"users\" where \"id\" is null and \"expires_at\" is null"
    );
    assert_eq!(
        users(sqlite())
            .where_("id", 1)
            .or_where_not_null("id")
            .to_sql(),
        "select * from \"users\" where \"id\" = ? or \"id\" is not null"
    );
}

#[test]
fn where_in_variants() {
    let query = users(sqlite()).where_in("id", [1, 2, 3]);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" in (?, ?, ?)"
    );
    assert_eq!(query.get_bindings(), vec![json!(1), json!(2), json!(3)]);

    let query = users(sqlite())
        .where_("id", 1)
        .or_where_not_in("id", vec!["a", "b"]);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" = ? or \"id\" not in (?, ?)"
    );

    assert_eq!(
        users(sqlite()).where_in("id", Vec::<i64>::new()).to_sql(),
        "select * from \"users\" where 0 = 1"
    );
    assert_eq!(
        users(sqlite())
            .where_not_in("id", Vec::<i64>::new())
            .to_sql(),
        "select * from \"users\" where 1 = 1"
    );

    let query = users(sqlite()).where_in("id", |q: Builder| {
        q.select("id")
            .from("users")
            .where_op("age", ">", 25)
            .take(3)
    });
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" in (select \"id\" from \"users\" where \"age\" > ? limit 3)"
    );
    assert_eq!(query.get_bindings(), vec![json!(25)]);

    let sub = sqlite()
        .table("posts")
        .select("user_id")
        .where_("active", true);
    let query = users(sqlite()).where_not_in("id", sub);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" not in (select \"user_id\" from \"posts\" where \"active\" = ?)"
    );
    assert_eq!(query.get_bindings(), vec![json!(true)]);

    let query = users(sqlite()).where_integer_in_raw("id", vec![1, 2, 3]);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" in (1, 2, 3)"
    );
    assert!(query.get_bindings().is_empty());
    assert_eq!(
        users(sqlite())
            .where_integer_not_in_raw("id", Vec::<i64>::new())
            .to_sql(),
        "select * from \"users\" where 1 = 1"
    );
}

#[test]
fn where_between_variants() {
    let query = users(sqlite()).where_between("id", [1, 2]);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" between ? and ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(1), json!(2)]);

    let query = users(sqlite()).where_not_between("id", (1, "2"));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" not between ? and ?"
    );

    let query = users(mysql())
        .where_("id", 1)
        .or_where_between("votes", vec![3, 4, 5]);
    assert_eq!(
        query.to_sql(),
        "select * from `users` where `id` = ? or `votes` between ? and ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(1), json!(3), json!(5)]);

    let query = users(sqlite()).where_between_columns(
        "weight",
        ["minimum_allowed_weight", "maximum_allowed_weight"],
    );
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"weight\" between \"minimum_allowed_weight\" and \"maximum_allowed_weight\""
    );
    let query = users(sqlite()).where_not_between_columns("weight", ["a", "b"]);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"weight\" not between \"a\" and \"b\""
    );
}

#[test]
fn where_column() {
    assert_eq!(
        users(sqlite())
            .where_column("first_name", "last_name")
            .or_where_column("first_name", "middle_name")
            .to_sql(),
        "select * from \"users\" where \"first_name\" = \"last_name\" or \"first_name\" = \"middle_name\""
    );
    assert_eq!(
        users(mysql())
            .where_column_op("updated_at", ">", "created_at")
            .to_sql(),
        "select * from `users` where `updated_at` > `created_at`"
    );
}

#[test]
fn raw_wheres() {
    let query = users(sqlite()).where_raw("id = ? or email = ?", (1, "foo"));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where id = ? or email = ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(1), json!("foo")]);

    let query = users(sqlite())
        .where_("id", 1)
        .or_where_raw("email = ?", ("foo",));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" = ? or email = ?"
    );
}

#[test]
fn nested_where_groups() {
    let query = users(sqlite())
        .where_("email", "foo")
        .or_where_group(|q| q.where_("name", "bar").where_("age", 25));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"email\" = ? or (\"name\" = ? and \"age\" = ?)"
    );
    assert_eq!(
        query.get_bindings(),
        vec![json!("foo"), json!("bar"), json!(25)]
    );

    let query = users(sqlite()).where_group(|q| q).where_("id", 1);
    assert_eq!(query.to_sql(), "select * from \"users\" where \"id\" = ?");

    let query =
        users(sqlite()).where_not(|q| q.where_("clearance", true).or_where_op("price", "<", 10));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where not (\"clearance\" = ? or \"price\" < ?)"
    );

    let query = users(sqlite())
        .where_("a", 1)
        .or_where_not(|q| q.where_("b", 2));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"a\" = ? or not (\"b\" = ?)"
    );
}

#[test]
fn where_any_all_none() {
    let query =
        users(sqlite())
            .where_("active", true)
            .where_any(["name", "email"], "like", "Example%");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"active\" = ? and (\"name\" like ? or \"email\" like ?)"
    );
    assert_eq!(
        query.get_bindings(),
        vec![json!(true), json!("Example%"), json!("Example%")]
    );

    let query = users(sqlite()).where_all(["title", "content"], "like", "%Laravel%");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where (\"title\" like ? and \"content\" like ?)"
    );

    let query = users(sqlite()).where_none(["name", "email"], "=", "x");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where not (\"name\" = ? or \"email\" = ?)"
    );

    let query = users(sqlite())
        .where_("a", 1)
        .or_where_any(["b", "c"], "=", 2);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"a\" = ? or (\"b\" = ? or \"c\" = ?)"
    );
}

#[test]
fn where_map_groups_equalities() {
    let query = users(sqlite()).where_map(json!({"name": "Taylor", "votes": 5}));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where (\"name\" = ? and \"votes\" = ?)"
    );
    assert_eq!(query.get_bindings(), vec![json!("Taylor"), json!(5)]);
}

#[test]
fn date_based_wheres() {
    let query = users(sqlite()).where_date("created_at", "2015-12-21");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where strftime('%Y-%m-%d', \"created_at\") = cast(? as text)"
    );
    assert_eq!(query.get_bindings(), vec![json!("2015-12-21")]);

    let query = users(sqlite()).where_day("created_at", 1);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where strftime('%d', \"created_at\") = cast(? as text)"
    );
    assert_eq!(query.get_bindings(), vec![json!("01")]);

    let query = users(sqlite()).where_month_op("created_at", ">", 5);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where strftime('%m', \"created_at\") > cast(? as text)"
    );
    assert_eq!(query.get_bindings(), vec![json!("05")]);

    assert_eq!(
        users(sqlite()).where_year("created_at", 2014).to_sql(),
        "select * from \"users\" where strftime('%Y', \"created_at\") = cast(? as text)"
    );
    assert_eq!(
        users(sqlite()).where_time("created_at", "22:00").to_sql(),
        "select * from \"users\" where strftime('%H:%M:%S', \"created_at\") = cast(? as text)"
    );

    assert_eq!(
        users(mysql())
            .where_date("created_at", "2015-12-21")
            .to_sql(),
        "select * from `users` where date(`created_at`) = ?"
    );
    assert_eq!(
        users(mysql())
            .where_day("created_at", 1)
            .or_where_year("created_at", 2014)
            .to_sql(),
        "select * from `users` where day(`created_at`) = ? or year(`created_at`) = ?"
    );
    assert_eq!(
        users(mysql())
            .where_time_op("created_at", ">=", "22:00")
            .to_sql(),
        "select * from `users` where time(`created_at`) >= ?"
    );

    assert_eq!(
        users(pgsql())
            .where_date("created_at", "2015-12-21")
            .to_sql(),
        "select * from \"users\" where \"created_at\"::date = ?"
    );
    assert_eq!(
        users(pgsql()).where_time("created_at", "22:00").to_sql(),
        "select * from \"users\" where \"created_at\"::time = ?"
    );
    assert_eq!(
        users(pgsql()).where_month("created_at", 5).to_sql(),
        "select * from \"users\" where extract(month from \"created_at\") = ?"
    );
    assert_eq!(
        users(pgsql()).where_year("created_at", 2014).to_sql(),
        "select * from \"users\" where extract(year from \"created_at\") = ?"
    );
}

#[test]
fn exists_and_sub_query_wheres() {
    let query = sqlite().table("orders").where_exists(|q: Builder| {
        q.select("*")
            .from("products")
            .where_column("products.id", "orders.id")
    });
    assert_eq!(
        query.to_sql(),
        "select * from \"orders\" where exists (select * from \"products\" where \"products\".\"id\" = \"orders\".\"id\")"
    );

    let query = sqlite()
        .table("orders")
        .where_("id", 1)
        .or_where_not_exists(|q: Builder| q.select("*").from("products").where_("active", 1));
    assert_eq!(
        query.to_sql(),
        "select * from \"orders\" where \"id\" = ? or not exists (select * from \"products\" where \"active\" = ?)"
    );
    assert_eq!(query.get_bindings(), vec![json!(1), json!(1)]);

    let query = users(sqlite()).where_sub("id", "=", |q: Builder| {
        q.select(raw("max(id)"))
            .from("users")
            .where_("email", "bar")
    });
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" = (select max(id) from \"users\" where \"email\" = ?)"
    );
    assert_eq!(query.get_bindings(), vec![json!("bar")]);

    let query = users(sqlite()).where_sub_value(
        |q: Builder| {
            q.select("type")
                .from("membership")
                .where_column("membership.user_id", "users.id")
                .limit(1)
        },
        "=",
        "Pro",
    );
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where (select \"type\" from \"membership\" where \"membership\".\"user_id\" = \"users\".\"id\" limit 1) = ?"
    );
}

#[test]
fn like_wheres_per_driver() {
    assert_eq!(
        users(sqlite()).where_like("name", "%Jo%").to_sql(),
        "select * from \"users\" where \"name\" like ?"
    );
    let query = users(sqlite()).where_like_case_sensitive("name", "Jo%_*");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"name\" glob ?"
    );
    assert_eq!(query.get_bindings(), vec![json!("Jo*?[*]")]);
    assert_eq!(
        users(sqlite()).where_not_like("name", "x").to_sql(),
        "select * from \"users\" where \"name\" not like ?"
    );

    assert_eq!(
        users(mysql()).where_like("name", "x").to_sql(),
        "select * from `users` where `name` like ?"
    );
    assert_eq!(
        users(mysql())
            .where_like_case_sensitive("name", "x")
            .to_sql(),
        "select * from `users` where `name` like binary ?"
    );

    assert_eq!(
        users(pgsql()).where_like("name", "x").to_sql(),
        "select * from \"users\" where \"name\"::text ilike ?"
    );
    assert_eq!(
        users(pgsql())
            .where_like_case_sensitive("name", "x")
            .to_sql(),
        "select * from \"users\" where \"name\"::text like ?"
    );
    assert_eq!(
        users(pgsql()).where_op("name", "ilike", "x").to_sql(),
        "select * from \"users\" where \"name\"::text ilike ?"
    );
}

#[test]
fn json_selectors_per_driver() {
    assert_eq!(
        users(sqlite())
            .select("items->price")
            .where_("items->price", 1)
            .to_sql(),
        "select json_extract(\"items\", '$.\"price\"') from \"users\" where json_extract(\"items\", '$.\"price\"') = ?"
    );
    assert_eq!(
        users(mysql())
            .select("items->price")
            .where_("items->price", 1)
            .to_sql(),
        "select json_unquote(json_extract(`items`, '$.\"price\"')) from `users` where json_unquote(json_extract(`items`, '$.\"price\"')) = ?"
    );
    assert_eq!(
        users(mariadb()).where_("items->price", 1).to_sql(),
        "select * from `users` where json_value(`items`, '$.\"price\"') = ?"
    );
    assert_eq!(
        users(pgsql())
            .select("items->price->in_usd")
            .where_("items->price", 1)
            .to_sql(),
        "select \"items\"->'price'->>'in_usd' from \"users\" where \"items\"->>'price' = ?"
    );
    assert_eq!(
        users(pgsql()).where_("items->languages[0]", "en").to_sql(),
        "select * from \"users\" where \"items\"->'languages'->>0 = ?"
    );
    assert_eq!(
        users(mysql()).where_("items->languages[0]", "en").to_sql(),
        "select * from `users` where json_unquote(json_extract(`items`, '$.\"languages\"[0]')) = ?"
    );
}

#[test]
fn json_boolean_wheres() {
    let query = users(mysql()).where_("options->enabled", true);
    assert_eq!(
        query.to_sql(),
        "select * from `users` where json_extract(`options`, '$.\"enabled\"') = true"
    );
    assert!(query.get_bindings().is_empty());

    assert_eq!(
        users(pgsql()).where_("options->enabled", false).to_sql(),
        "select * from \"users\" where (\"options\"->'enabled')::jsonb = 'false'::jsonb"
    );
    assert_eq!(
        users(sqlite()).where_("options->enabled", true).to_sql(),
        "select * from \"users\" where json_extract(\"options\", '$.\"enabled\"') = true"
    );
}

#[test]
fn json_contains_key_and_length() {
    let query = users(mysql()).where_json_contains("options->languages", json!(["en"]));
    assert_eq!(
        query.to_sql(),
        "select * from `users` where json_contains(`options`, ?, '$.\"languages\"')"
    );
    assert_eq!(query.get_bindings(), vec![json!("[\"en\"]")]);

    let query = users(pgsql()).where_json_contains("options->languages", "en");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where (\"options\"->'languages')::jsonb @> ?"
    );
    assert_eq!(query.get_bindings(), vec![json!("\"en\"")]);

    let query = users(sqlite()).where_json_doesnt_contain("options->languages", "en");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where not exists (select 1 from json_each(\"options\", '$.\"languages\"') where \"json_each\".\"value\" is ?)"
    );
    assert_eq!(query.get_bindings(), vec![json!("en")]);

    assert_eq!(
        users(sqlite())
            .where_json_contains_key("options->languages")
            .to_sql(),
        "select * from \"users\" where json_type(\"options\", '$.\"languages\"') is not null"
    );
    assert_eq!(
        users(mysql())
            .where_json_contains_key("options->languages")
            .to_sql(),
        "select * from `users` where ifnull(json_contains_path(`options`, 'one', '$.\"languages\"'), 0)"
    );
    assert_eq!(
        users(pgsql())
            .where_json_contains_key("options->languages")
            .to_sql(),
        "select * from \"users\" where coalesce((\"options\")::jsonb ?? 'languages', false)"
    );

    assert_eq!(
        users(mysql())
            .where_json_length_op("options->languages", ">", 1)
            .to_sql(),
        "select * from `users` where json_length(`options`, '$.\"languages\"') > ?"
    );
    assert_eq!(
        users(pgsql())
            .where_json_length("options->languages", 0)
            .to_sql(),
        "select * from \"users\" where jsonb_array_length((\"options\"->'languages')::jsonb) = ?"
    );
    assert_eq!(
        users(sqlite())
            .where_json_length("options->languages", 0)
            .to_sql(),
        "select * from \"users\" where json_array_length(\"options\", '$.\"languages\"') = ?"
    );
}

#[test]
fn null_json_selectors_on_mysql() {
    assert_eq!(
        users(mysql()).where_null("items->id").to_sql(),
        "select * from `users` where (json_extract(`items`, '$.\"id\"') is null OR json_type(json_extract(`items`, '$.\"id\"')) = 'NULL')"
    );
}

#[test]
fn fulltext_wheres() {
    assert_eq!(
        users(mysql())
            .where_fulltext("body", "Hello World")
            .to_sql(),
        "select * from `users` where match (`body`) against (? in natural language mode)"
    );
    assert_eq!(
        users(mysql())
            .where_fulltext_with(
                ["body", "title"],
                "Hello",
                FullTextOptions {
                    mode: Some("boolean".into()),
                    ..Default::default()
                }
            )
            .to_sql(),
        "select * from `users` where match (`body`, `title`) against (? in boolean mode)"
    );
    assert_eq!(
        users(pgsql())
            .where_fulltext("body", "Hello World")
            .to_sql(),
        "select * from \"users\" where (to_tsvector('english', \"body\")) @@ plainto_tsquery('english', ?)"
    );
    assert!(
        users(sqlite())
            .where_fulltext("body", "x")
            .try_to_sql()
            .is_err()
    );
}

#[test]
fn joins() {
    let query = users(sqlite())
        .join("contacts", "users.id", "=", "contacts.id")
        .left_join("photos", "users.id", "=", "photos.id");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" inner join \"contacts\" on \"users\".\"id\" = \"contacts\".\"id\" left join \"photos\" on \"users\".\"id\" = \"photos\".\"id\""
    );

    let query = users(mysql()).right_join("photos", "users.id", "=", "photos.user_id");
    assert_eq!(
        query.to_sql(),
        "select * from `users` right join `photos` on `users`.`id` = `photos`.`user_id`"
    );

    assert_eq!(
        sqlite().table("sizes").cross_join("colors").to_sql(),
        "select * from \"sizes\" cross join \"colors\""
    );

    let query = users(sqlite()).join_where("contacts", "contacts.status", "=", "active");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" inner join \"contacts\" on \"contacts\".\"status\" = ?"
    );
    assert_eq!(query.get_bindings(), vec![json!("active")]);
}

#[test]
fn complex_joins_with_closures() {
    let query = users(sqlite()).join_with("contacts", |join: JoinClause| {
        join.on("users.id", "=", "contacts.id")
            .or_on("users.name", "=", "contacts.name")
    });
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" inner join \"contacts\" on \"users\".\"id\" = \"contacts\".\"id\" or \"users\".\"name\" = \"contacts\".\"name\""
    );

    let query = users(sqlite())
        .left_join_with("contacts", |join: JoinClause| {
            join.on("users.id", "=", "contacts.id")
                .where_("contacts.active", true)
                .where_null("contacts.deleted_at")
        })
        .where_("users.id", 5);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" left join \"contacts\" on \"users\".\"id\" = \"contacts\".\"id\" and \"contacts\".\"active\" = ? and \"contacts\".\"deleted_at\" is null where \"users\".\"id\" = ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(true), json!(5)]);

    let query = users(sqlite()).join_with("contacts", |join: JoinClause| {
        join.on("users.id", "=", "contacts.id")
            .on_group(|nested| nested.on("a", "=", "b").or_on("c", "=", "d"))
    });
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" inner join \"contacts\" on \"users\".\"id\" = \"contacts\".\"id\" and (\"a\" = \"b\" or \"c\" = \"d\")"
    );
}

#[test]
fn sub_query_joins() {
    let latest = sqlite()
        .table("posts")
        .select(["user_id", "created_at as last_post"])
        .where_("published", true);
    let query = users(sqlite()).join_sub(
        latest,
        "latest_posts",
        "users.id",
        "=",
        "latest_posts.user_id",
    );
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" inner join (select \"user_id\", \"created_at\" as \"last_post\" from \"posts\" where \"published\" = ?) as \"latest_posts\" on \"users\".\"id\" = \"latest_posts\".\"user_id\""
    );
    assert_eq!(query.get_bindings(), vec![json!(true)]);

    let query =
        users(mysql()).left_join_sub("select * from contacts", "sub", "users.id", "=", "sub.id");
    assert_eq!(
        query.to_sql(),
        "select * from `users` left join (select * from contacts) as `sub` on `users`.`id` = `sub`.`id`"
    );

    let query = users(sqlite()).cross_join_sub(|q: Builder| q.from("sizes"), "s");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" cross join (select * from \"sizes\") as \"s\""
    );
}

#[test]
fn join_bindings_come_before_where_bindings() {
    let query = users(sqlite())
        .where_("id", 1)
        .join_with("contacts", |join: JoinClause| {
            join.on("users.id", "=", "contacts.id")
                .where_("contacts.x", "y")
        });
    assert_eq!(query.get_bindings(), vec![json!("y"), json!(1)]);
}

#[test]
fn sub_selects_and_from_subs() {
    let query = users(sqlite()).select(["name"]).select_sub(
        |q: Builder| {
            q.from("posts")
                .select_raw("count(*)", ())
                .where_column("posts.user_id", "users.id")
        },
        "post_count",
    );
    assert_eq!(
        query.to_sql(),
        "select \"name\", (select count(*) from \"posts\" where \"posts\".\"user_id\" = \"users\".\"id\") as \"post_count\" from \"users\""
    );

    let query = sqlite()
        .query()
        .from_sub(
            |q: Builder| {
                q.from("user_sessions")
                    .select_raw("max(last_seen_at) as last_seen_at", ())
                    .where_("active", 1)
            },
            "sessions",
        )
        .where_op("last_seen_at", ">", "2024-01-01");
    assert_eq!(
        query.to_sql(),
        "select * from (select max(last_seen_at) as last_seen_at from \"user_sessions\" where \"active\" = ?) as \"sessions\" where \"last_seen_at\" > ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(1), json!("2024-01-01")]);

    let query = sqlite()
        .query()
        .from_raw("(select * from users) as u where u.id > ?", (5,));
    assert_eq!(
        query.to_sql(),
        "select * from (select * from users) as u where u.id > ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(5)]);
}

#[test]
fn grouping_and_having() {
    let query = users(sqlite()).group_by("email").having_op("email", ">", 1);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" group by \"email\" having \"email\" > ?"
    );

    let query = users(sqlite())
        .group_by(["first_name", "status"])
        .having("status", 1)
        .or_having_op("votes", ">", 5);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" group by \"first_name\", \"status\" having \"status\" = ? or \"votes\" > ?"
    );

    let query = users(mysql())
        .select_raw("count(*) as total", ())
        .group_by_raw("year(created_at)", ())
        .having_raw("count(*) > ?", (2,))
        .or_having_raw("sum(votes) > ?", (10,));
    assert_eq!(
        query.to_sql(),
        "select count(*) as total from `users` group by year(created_at) having count(*) > ? or sum(votes) > ?"
    );
    assert_eq!(query.get_bindings(), vec![json!(2), json!(10)]);

    let query = users(sqlite())
        .group_by("last_name")
        .having_between("last_login_date", ["2018-11-16", "2018-12-16"]);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" group by \"last_name\" having \"last_login_date\" between ? and ?"
    );

    let query = users(sqlite())
        .group_by("x")
        .having_null("y")
        .having_not_null("z");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" group by \"x\" having \"y\" is null and \"z\" is not null"
    );

    let query = users(sqlite())
        .group_by("x")
        .having_group(|q| q.having_op("a", ">", 1).or_having_op("b", "<", 2));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" group by \"x\" having (\"a\" > ? or \"b\" < ?)"
    );

    let query = users(pgsql()).group_by("x").having_op("flags", "&", 4);
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" group by \"x\" having (\"flags\" & ?)::bool"
    );
}

#[test]
fn ordering() {
    let query = users(sqlite())
        .order_by("email", "asc")
        .order_by_desc("age");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" order by \"email\" asc, \"age\" desc"
    );

    assert_eq!(
        users(sqlite()).latest().to_sql(),
        "select * from \"users\" order by \"created_at\" desc"
    );
    assert_eq!(
        users(sqlite()).oldest_by("updated_at").to_sql(),
        "select * from \"users\" order by \"updated_at\" asc"
    );

    let query = users(sqlite()).order_by("email", "asc").reorder();
    assert_eq!(query.to_sql(), "select * from \"users\"");
    let query = users(sqlite())
        .order_by("email", "asc")
        .reorder_by("name", "desc");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" order by \"name\" desc"
    );

    let query = users(sqlite())
        .order_by_raw("updated_at - created_at DESC", ())
        .order_by("name", "asc");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" order by updated_at - created_at DESC, \"name\" asc"
    );

    assert!(
        users(sqlite())
            .order_by("name", "sideways")
            .try_to_sql()
            .is_err()
    );
}

#[test]
fn random_ordering_per_driver() {
    assert_eq!(
        users(sqlite()).in_random_order().to_sql(),
        "select * from \"users\" order by RANDOM()"
    );
    assert_eq!(
        users(pgsql()).in_random_order().to_sql(),
        "select * from \"users\" order by RANDOM()"
    );
    assert_eq!(
        users(mysql()).in_random_order().to_sql(),
        "select * from `users` order by RAND()"
    );
    assert_eq!(
        users(mysql()).in_random_order_seed(42).to_sql(),
        "select * from `users` order by RAND(42)"
    );
}

#[test]
fn limits_offsets_and_pages() {
    assert_eq!(
        users(sqlite()).limit(10).offset(5).to_sql(),
        "select * from \"users\" limit 10 offset 5"
    );
    assert_eq!(
        users(sqlite()).take(10).skip(5).to_sql(),
        "select * from \"users\" limit 10 offset 5"
    );
    assert_eq!(
        users(sqlite()).for_page(2, 15).to_sql(),
        "select * from \"users\" limit 15 offset 15"
    );
    assert_eq!(
        users(sqlite()).for_page(0, 15).to_sql(),
        "select * from \"users\" limit 15 offset 0"
    );
    assert_eq!(
        users(sqlite()).limit(-1).to_sql(),
        "select * from \"users\""
    );

    let query = users(sqlite())
        .order_by("id", "asc")
        .for_page_after_id(15, 1, "id");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" > ? order by \"id\" asc limit 15"
    );
    let query = users(sqlite()).for_page_before_id(15, Value::Null, "id");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"id\" is not null order by \"id\" desc limit 15"
    );
}

#[test]
fn unions() {
    let query = users(mysql())
        .where_("id", 1)
        .union(mysql().table("users").where_("id", 2));
    assert_eq!(
        query.to_sql(),
        "(select * from `users` where `id` = ?) union (select * from `users` where `id` = ?)"
    );
    assert_eq!(query.get_bindings(), vec![json!(1), json!(2)]);

    let query = users(sqlite())
        .where_("id", 1)
        .union_all(|q: Builder| q.from("users").where_("id", 2));
    assert_eq!(
        query.to_sql(),
        "select * from (select * from \"users\" where \"id\" = ?) union all select * from (select * from \"users\" where \"id\" = ?)"
    );

    let query = users(pgsql())
        .select("name")
        .union(pgsql().table("admins").select("name"))
        .order_by("name", "asc")
        .limit(10)
        .offset(5);
    assert_eq!(
        query.to_sql(),
        "(select \"name\" from \"users\") union (select \"name\" from \"admins\") order by \"name\" asc limit 10 offset 5"
    );
}

#[test]
fn locks_per_driver() {
    assert_eq!(
        users(mysql()).where_("id", 1).lock_for_update().to_sql(),
        "select * from `users` where `id` = ? for update"
    );
    assert_eq!(
        users(mysql()).shared_lock().to_sql(),
        "select * from `users` lock in share mode"
    );
    assert_eq!(
        users(pgsql()).lock_for_update().to_sql(),
        "select * from \"users\" for update"
    );
    assert_eq!(
        users(pgsql()).shared_lock().to_sql(),
        "select * from \"users\" for share"
    );
    assert_eq!(
        users(sqlite()).lock_for_update().to_sql(),
        "select * from \"users\""
    );
    assert_eq!(
        users(pgsql())
            .lock(Lock::Raw("for update skip locked".into()))
            .to_sql(),
        "select * from \"users\" for update skip locked"
    );
}

#[test]
fn conditional_clauses() {
    let is_admin = true;
    let query = users(sqlite())
        .when(is_admin, |q| q.where_("role", "admin"))
        .unless(true, |q| q.where_("never", 1))
        .when_some(Some(5), |q, votes| q.where_op("votes", ">", votes));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"role\" = ? and \"votes\" > ?"
    );

    use illuminate_support::Tappable;
    let query = users(sqlite()).tap(|q| *q = q.clone().where_("id", 1));
    assert_eq!(query.to_sql(), "select * from \"users\" where \"id\" = ?");
}

#[test]
fn raw_sql_embeds_bindings() {
    let query = users(sqlite())
        .where_("email", "foo")
        .where_("id", 1)
        .where_("admin", true);
    assert_eq!(
        query.to_raw_sql(),
        "select * from \"users\" where \"email\" = 'foo' and \"id\" = 1 and \"admin\" = 1"
    );
    let query = users(pgsql())
        .where_("name", "O'Brien")
        .where_("active", false);
    assert_eq!(
        query.to_raw_sql(),
        "select * from \"users\" where \"name\" = 'O''Brien' and \"active\" = false"
    );
    let query = users(mysql()).where_("name", "?").where_("id", Value::Null);
    assert_eq!(
        query.to_raw_sql(),
        "select * from `users` where `name` = '?' and `id` is null"
    );
}

#[test]
fn clone_without_parts() {
    let query = users(sqlite())
        .select("x")
        .where_("a", 1)
        .order_by("x", "asc")
        .limit(5)
        .offset(2);
    let clone = query.clone_without(&["columns", "orders", "limit", "offset"]);
    assert_eq!(clone.to_sql(), "select * from \"users\" where \"a\" = ?");
    assert_eq!(
        query.to_sql(),
        "select \"x\" from \"users\" where \"a\" = ? order by \"x\" asc limit 5 offset 2"
    );

    let query = users(sqlite()).select_raw("? as x", (1,)).where_("a", 2);
    let clone = query.clone_without_bindings(&[illuminate_database::query::BindingType::Select]);
    assert_eq!(clone.get_bindings(), vec![json!(2)]);
}

#[test]
fn builders_expose_their_parts() {
    let query = users(sqlite())
        .select(["id"])
        .where_("a", 1)
        .order_by("id", "desc")
        .limit(3)
        .offset(1);
    assert_eq!(query.table_name(), Some("users"));
    assert_eq!(query.get_columns(), vec!["id"]);
    assert_eq!(query.wheres.len(), 1);
    assert_eq!(query.orders.len(), 1);
    assert_eq!(query.get_limit(), Some(3));
    assert_eq!(query.get_offset(), Some(1));
    assert_eq!(query.get_raw_bindings().where_, vec![json!(1)]);
}

#[test]
fn raw_expressions_as_columns_and_values() {
    let query = users(sqlite()).where_("created_at", Expression::new("CURRENT_TIMESTAMP"));
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where \"created_at\" = CURRENT_TIMESTAMP"
    );
    assert!(query.get_bindings().is_empty());

    let query = users(sqlite()).where_op(raw("lower(name)"), "=", "taylor");
    assert_eq!(
        query.to_sql(),
        "select * from \"users\" where lower(name) = ?"
    );
}

#[test]
fn bitwise_and_null_safe_operators() {
    assert_eq!(
        users(pgsql()).where_op("flags", "&", 4).to_sql(),
        "select * from \"users\" where (\"flags\" & ?)::bool"
    );
    assert_eq!(
        users(mysql()).where_op("flags", "&", 4).to_sql(),
        "select * from `users` where `flags` & ?"
    );
    assert_eq!(
        users(mysql()).where_op("a", "<=>", 1).to_sql(),
        "select * from `users` where `a` <=> ?"
    );
    assert_eq!(
        users(sqlite()).where_op("a", "<=>", 1).to_sql(),
        "select * from \"users\" where \"a\" is ?"
    );
    assert_eq!(
        users(pgsql()).where_op("a", "<=>", 1).to_sql(),
        "select * from \"users\" where \"a\" is not distinct from ?"
    );
    assert_eq!(
        users(pgsql()).where_op("tags", "?|", "a").to_sql(),
        "select * from \"users\" where \"tags\" ??| ?"
    );
}

#[test]
fn postgres_question_mark_operators_survive_raw_sql() {
    let query = users(pgsql()).where_op("tags", "?|", "a");
    assert_eq!(
        query.to_raw_sql(),
        "select * from \"users\" where \"tags\" ?| 'a'"
    );
}

#[test]
fn mysql_and_mariadb_share_a_dialect() {
    assert_eq!(
        users(mariadb()).where_("id", 1).to_sql(),
        "select * from `users` where `id` = ?"
    );
}
