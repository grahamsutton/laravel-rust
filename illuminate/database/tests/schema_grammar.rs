//! DDL generation tests for the SQLite, MySQL and PostgreSQL schema grammars.

use illuminate_database::schema::{Blueprint, SchemaGrammar};
use illuminate_database::{Driver, Expression};
use illuminate_support::{Value, json};

fn sqlite() -> SchemaGrammar {
    SchemaGrammar::new(Driver::Sqlite, "", Value::Null)
}

fn mysql() -> SchemaGrammar {
    SchemaGrammar::new(Driver::MySql, "", Value::Null)
}

fn mysql_with(config: Value) -> SchemaGrammar {
    SchemaGrammar::new(Driver::MySql, "", config)
}

fn pgsql() -> SchemaGrammar {
    SchemaGrammar::new(Driver::Postgres, "", Value::Null)
}

fn sql(grammar: &SchemaGrammar, blueprint: &mut Blueprint) -> Vec<String> {
    blueprint.to_sql(grammar).unwrap()
}

fn create(build: impl Fn(&mut Blueprint)) -> Blueprint {
    let mut blueprint = Blueprint::creating("users");
    build(&mut blueprint);
    blueprint
}

fn alter(build: impl Fn(&mut Blueprint)) -> Blueprint {
    let mut blueprint = Blueprint::new("users");
    build(&mut blueprint);
    blueprint
}

#[test]
fn basic_create_tables() {
    let build = |t: &mut Blueprint| {
        t.increments("id");
        t.string("email");
    };
    assert_eq!(
        sql(&sqlite(), &mut create(build)),
        vec![
            "create table \"users\" (\"id\" integer primary key autoincrement not null, \"email\" varchar not null)"
        ]
    );
    assert_eq!(
        sql(&mysql(), &mut create(build)),
        vec![
            "create table `users` (`id` int unsigned not null auto_increment primary key, `email` varchar(255) not null)"
        ]
    );
    assert_eq!(
        sql(&pgsql(), &mut create(build)),
        vec![
            "create table \"users\" (\"id\" serial not null primary key, \"email\" varchar(255) not null)"
        ]
    );
}

#[test]
fn mysql_create_uses_charset_collation_and_engine() {
    let grammar = mysql_with(
        json!({"charset": "utf8mb4", "collation": "utf8mb4_unicode_ci", "engine": "InnoDB"}),
    );
    let mut blueprint = create(|t| {
        t.id();
    });
    assert_eq!(
        sql(&grammar, &mut blueprint),
        vec![
            "create table `users` (`id` bigint unsigned not null auto_increment primary key) default character set utf8mb4 collate 'utf8mb4_unicode_ci' engine = InnoDB"
        ]
    );

    let mut blueprint = create(|t| {
        t.id();
    });
    blueprint.charset = Some("utf8".into());
    blueprint.collation = Some("utf8_bin".into());
    blueprint.engine("MyISAM");
    assert_eq!(
        sql(&mysql(), &mut blueprint),
        vec![
            "create table `users` (`id` bigint unsigned not null auto_increment primary key) default character set utf8 collate 'utf8_bin' engine = MyISAM"
        ]
    );
}

#[test]
fn temporary_tables() {
    let mut blueprint = create(|t| {
        t.string("name");
    });
    blueprint.temporary();
    assert_eq!(
        sql(&sqlite(), &mut blueprint),
        vec!["create temporary table \"users\" (\"name\" varchar not null)"]
    );
}

#[test]
fn ids_and_timestamps() {
    let build = |t: &mut Blueprint| {
        t.id();
        t.timestamps();
        t.soft_deletes();
        t.remember_token();
    };
    assert_eq!(
        sql(&sqlite(), &mut create(build)),
        vec![
            "create table \"users\" (\"id\" integer primary key autoincrement not null, \"created_at\" datetime, \"updated_at\" datetime, \"deleted_at\" datetime, \"remember_token\" varchar)"
        ]
    );
    assert_eq!(
        sql(&mysql(), &mut create(build)),
        vec![
            "create table `users` (`id` bigint unsigned not null auto_increment primary key, `created_at` timestamp null, `updated_at` timestamp null, `deleted_at` timestamp null, `remember_token` varchar(100) null)"
        ]
    );
    assert_eq!(
        sql(&pgsql(), &mut create(build)),
        vec![
            "create table \"users\" (\"id\" bigserial not null primary key, \"created_at\" timestamp(0) without time zone null, \"updated_at\" timestamp(0) without time zone null, \"deleted_at\" timestamp(0) without time zone null, \"remember_token\" varchar(100) null)"
        ]
    );
}

#[test]
fn column_types_per_driver() {
    let build = |t: &mut Blueprint| {
        t.char_len("code", 4);
        t.text("bio");
        t.long_text("body");
        t.big_integer("views");
        t.unsigned_tiny_integer("level");
        t.small_integer("rank");
        t.float("ratio");
        t.double("score");
        t.decimal("amount", 8, 2);
        t.boolean("active");
        t.enum_("role", ["member", "admin"]);
        t.json("options");
        t.jsonb("meta");
        t.date("born_on");
        t.date_time("seen_at");
        t.time("opens_at");
        t.timestamp_tz("signed_at");
        t.year("graduated");
        t.binary("photo");
        t.uuid("uuid");
        t.ulid("ulid");
        t.ip_address("ip");
        t.mac_address("mac");
    };

    assert_eq!(
        sql(&sqlite(), &mut create(build)),
        vec![concat!(
            "create table \"users\" (\"code\" varchar not null, \"bio\" text not null, \"body\" text not null, ",
            "\"views\" integer not null, \"level\" integer not null, \"rank\" integer not null, \"ratio\" float not null, ",
            "\"score\" double not null, \"amount\" numeric not null, \"active\" tinyint(1) not null, ",
            "\"role\" varchar check (\"role\" in ('member', 'admin')) not null, \"options\" text not null, \"meta\" text not null, ",
            "\"born_on\" date not null, \"seen_at\" datetime not null, \"opens_at\" time not null, \"signed_at\" datetime not null, ",
            "\"graduated\" integer not null, \"photo\" blob not null, \"uuid\" varchar not null, \"ulid\" varchar not null, ",
            "\"ip\" varchar not null, \"mac\" varchar not null)"
        )]
    );

    assert_eq!(
        sql(&mysql(), &mut create(build)),
        vec![concat!(
            "create table `users` (`code` char(4) not null, `bio` text not null, `body` longtext not null, ",
            "`views` bigint not null, `level` tinyint unsigned not null, `rank` smallint not null, `ratio` float(53) not null, ",
            "`score` double not null, `amount` decimal(8, 2) not null, `active` tinyint(1) not null, ",
            "`role` enum('member', 'admin') not null, `options` json not null, `meta` json not null, ",
            "`born_on` date not null, `seen_at` datetime not null, `opens_at` time not null, `signed_at` timestamp not null, ",
            "`graduated` year not null, `photo` blob not null, `uuid` char(36) not null, `ulid` char(26) not null, ",
            "`ip` varchar(45) not null, `mac` varchar(17) not null)"
        )]
    );

    assert_eq!(
        sql(&pgsql(), &mut create(build)),
        vec![concat!(
            "create table \"users\" (\"code\" char(4) not null, \"bio\" text not null, \"body\" text not null, ",
            "\"views\" bigint not null, \"level\" smallint not null, \"rank\" smallint not null, \"ratio\" float(53) not null, ",
            "\"score\" double precision not null, \"amount\" decimal(8, 2) not null, \"active\" boolean not null, ",
            "\"role\" varchar(255) check (\"role\" in ('member', 'admin')) not null, \"options\" json not null, \"meta\" jsonb not null, ",
            "\"born_on\" date not null, \"seen_at\" timestamp(0) without time zone not null, \"opens_at\" time(0) without time zone not null, ",
            "\"signed_at\" timestamp(0) with time zone not null, \"graduated\" integer not null, \"photo\" bytea not null, ",
            "\"uuid\" uuid not null, \"ulid\" char(26) not null, \"ip\" inet not null, \"mac\" macaddr not null)"
        )]
    );
}

#[test]
fn defaults_and_modifiers() {
    let build = |t: &mut Blueprint| {
        t.string("name").nullable().default("O'Reilly");
        t.boolean("active").default(true);
        t.integer("votes")
            .unsigned()
            .default(0)
            .comment("It's votes");
        t.timestamp("created_at").use_current();
        t.timestamp("updated_at")
            .use_current()
            .use_current_on_update();
        t.string("status").default(Expression::new("'draft'"));
    };
    assert_eq!(
        sql(&sqlite(), &mut create(build)),
        vec![concat!(
            "create table \"users\" (\"name\" varchar default 'O''Reilly', \"active\" tinyint(1) not null default '1', ",
            "\"votes\" integer not null default '0', \"created_at\" datetime not null default CURRENT_TIMESTAMP, ",
            "\"updated_at\" datetime not null default CURRENT_TIMESTAMP, \"status\" varchar not null default 'draft')"
        )]
    );
    assert_eq!(
        sql(&mysql(), &mut create(build)),
        vec![concat!(
            "create table `users` (`name` varchar(255) null default 'O''Reilly', `active` tinyint(1) not null default '1', ",
            "`votes` int unsigned not null default '0' comment 'It\\'s votes', `created_at` timestamp not null default CURRENT_TIMESTAMP, ",
            "`updated_at` timestamp not null default CURRENT_TIMESTAMP on update CURRENT_TIMESTAMP, `status` varchar(255) not null default 'draft')"
        )]
    );
    assert_eq!(
        sql(&pgsql(), &mut create(build)),
        vec![
            concat!(
                "create table \"users\" (\"name\" varchar(255) null default 'O''Reilly', \"active\" boolean not null default '1', ",
                "\"votes\" integer not null default '0', \"created_at\" timestamp(0) without time zone not null default CURRENT_TIMESTAMP, ",
                "\"updated_at\" timestamp(0) without time zone not null default CURRENT_TIMESTAMP, \"status\" varchar(255) not null default 'draft')"
            )
            .to_string(),
            "comment on column \"users\".\"votes\" is 'It''s votes'".to_string(),
        ]
    );
}

#[test]
fn fluent_indexes() {
    let build = |t: &mut Blueprint| {
        t.id();
        t.string("email").unique();
        t.string("name").index();
        t.string("slug").unique_named("custom_slug");
    };
    assert_eq!(
        sql(&sqlite(), &mut create(build)),
        vec![
            "create table \"users\" (\"id\" integer primary key autoincrement not null, \"email\" varchar not null, \"name\" varchar not null, \"slug\" varchar not null)",
            "create unique index \"users_email_unique\" on \"users\" (\"email\")",
            "create index \"users_name_index\" on \"users\" (\"name\")",
            "create unique index \"custom_slug\" on \"users\" (\"slug\")",
        ]
    );
    assert_eq!(
        sql(&mysql(), &mut create(build))[1..],
        [
            "alter table `users` add unique `users_email_unique`(`email`)",
            "alter table `users` add index `users_name_index`(`name`)",
            "alter table `users` add unique `custom_slug`(`slug`)",
        ]
    );
    assert_eq!(
        sql(&pgsql(), &mut create(build))[1..],
        [
            "alter table \"users\" add constraint \"users_email_unique\" unique (\"email\")",
            "create index \"users_name_index\" on \"users\" (\"name\")",
            "alter table \"users\" add constraint \"custom_slug\" unique (\"slug\")",
        ]
    );
}

#[test]
fn composite_primary_keys() {
    let build = |t: &mut Blueprint| {
        t.string("a");
        t.string("b");
        t.primary(["a", "b"]);
    };
    assert_eq!(
        sql(&sqlite(), &mut create(build)),
        vec![
            "create table \"users\" (\"a\" varchar not null, \"b\" varchar not null, primary key (\"a\", \"b\"))"
        ]
    );
    assert_eq!(
        sql(&mysql(), &mut create(build)),
        vec![
            "create table `users` (`a` varchar(255) not null, `b` varchar(255) not null, primary key (`a`, `b`))"
        ]
    );
    assert_eq!(
        sql(&pgsql(), &mut create(build)),
        vec![
            "create table \"users\" (\"a\" varchar(255) not null, \"b\" varchar(255) not null)",
            "alter table \"users\" add primary key (\"a\", \"b\")",
        ]
    );
}

#[test]
fn foreign_keys() {
    let build = |t: &mut Blueprint| {
        t.id();
        t.foreign_id("user_id").constrained().cascade_on_delete();
        t.unsigned_big_integer("team_id").nullable();
        t.foreign("team_id")
            .references("id")
            .on("teams")
            .null_on_delete()
            .cascade_on_update();
    };
    let mut blueprint = Blueprint::creating("posts");
    build(&mut blueprint);
    assert_eq!(
        sql(&sqlite(), &mut blueprint),
        vec![concat!(
            "create table \"posts\" (\"id\" integer primary key autoincrement not null, \"user_id\" integer not null, \"team_id\" integer, ",
            "foreign key(\"user_id\") references \"users\"(\"id\") on delete cascade, ",
            "foreign key(\"team_id\") references \"teams\"(\"id\") on delete set null on update cascade)"
        )]
    );

    let mut blueprint = Blueprint::creating("posts");
    build(&mut blueprint);
    assert_eq!(
        sql(&mysql(), &mut blueprint),
        vec![
            "create table `posts` (`id` bigint unsigned not null auto_increment primary key, `user_id` bigint unsigned not null, `team_id` bigint unsigned null)",
            "alter table `posts` add constraint `posts_user_id_foreign` foreign key (`user_id`) references `users` (`id`) on delete cascade",
            "alter table `posts` add constraint `posts_team_id_foreign` foreign key (`team_id`) references `teams` (`id`) on delete set null on update cascade",
        ]
    );

    let mut blueprint = Blueprint::creating("posts");
    build(&mut blueprint);
    assert_eq!(
        sql(&pgsql(), &mut blueprint)[1..],
        [
            "alter table \"posts\" add constraint \"posts_user_id_foreign\" foreign key (\"user_id\") references \"users\" (\"id\") on delete cascade",
            "alter table \"posts\" add constraint \"posts_team_id_foreign\" foreign key (\"team_id\") references \"teams\" (\"id\") on delete set null on update cascade",
        ]
    );
}

#[test]
fn adding_columns() {
    let build = |t: &mut Blueprint| {
        t.string("phone").nullable();
        t.integer("votes").default(0);
    };
    assert_eq!(
        sql(&sqlite(), &mut alter(build)),
        vec![
            "alter table \"users\" add column \"phone\" varchar",
            "alter table \"users\" add column \"votes\" integer not null default '0'",
        ]
    );
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec![
            "alter table `users` add `phone` varchar(255) null",
            "alter table `users` add `votes` int not null default '0'",
        ]
    );
    assert_eq!(
        sql(&pgsql(), &mut alter(build)),
        vec![
            "alter table \"users\" add column \"phone\" varchar(255) null",
            "alter table \"users\" add column \"votes\" integer not null default '0'",
        ]
    );
}

#[test]
fn mysql_after_and_first() {
    let mut blueprint = alter(|t| {
        t.string("name").after("id");
        t.string("code").first();
    });
    assert_eq!(
        sql(&mysql(), &mut blueprint),
        vec![
            "alter table `users` add `name` varchar(255) not null after `id`",
            "alter table `users` add `code` varchar(255) not null first",
        ]
    );

    let mut blueprint = Blueprint::new("users");
    blueprint.after("password", |t| {
        t.string("address_line1");
        t.string("city");
    });
    assert_eq!(
        sql(&mysql(), &mut blueprint),
        vec![
            "alter table `users` add `address_line1` varchar(255) not null after `password`",
            "alter table `users` add `city` varchar(255) not null after `address_line1`",
        ]
    );
}

#[test]
fn dropping_tables_and_columns() {
    let mut blueprint = Blueprint::new("users");
    blueprint.drop();
    assert_eq!(sql(&sqlite(), &mut blueprint), vec!["drop table \"users\""]);
    let mut blueprint = Blueprint::new("users");
    blueprint.drop_if_exists();
    assert_eq!(
        sql(&mysql(), &mut blueprint),
        vec!["drop table if exists `users`"]
    );

    let build = |t: &mut Blueprint| t.drop_column(["foo", "bar"]);
    assert_eq!(
        sql(&sqlite(), &mut alter(build)),
        vec![
            "alter table \"users\" drop column \"foo\"",
            "alter table \"users\" drop column \"bar\""
        ]
    );
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec!["alter table `users` drop `foo`, drop `bar`"]
    );
    assert_eq!(
        sql(&pgsql(), &mut alter(build)),
        vec!["alter table \"users\" drop column \"foo\", drop column \"bar\""]
    );

    assert_eq!(
        sql(&mysql(), &mut alter(|t| t.drop_timestamps())),
        vec!["alter table `users` drop `created_at`, drop `updated_at`"]
    );
}

#[test]
fn renaming() {
    let build = |t: &mut Blueprint| t.rename("people");
    assert_eq!(
        sql(&sqlite(), &mut alter(build)),
        vec!["alter table \"users\" rename to \"people\""]
    );
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec!["rename table `users` to `people`"]
    );
    assert_eq!(
        sql(&pgsql(), &mut alter(build)),
        vec!["alter table \"users\" rename to \"people\""]
    );

    let build = |t: &mut Blueprint| t.rename_column("from", "to");
    assert_eq!(
        sql(&sqlite(), &mut alter(build)),
        vec!["alter table \"users\" rename column \"from\" to \"to\""]
    );
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec!["alter table `users` rename column `from` to `to`"]
    );

    let build = |t: &mut Blueprint| t.rename_index("foo", "bar");
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec!["alter table `users` rename index `foo` to `bar`"]
    );
    assert_eq!(
        sql(&pgsql(), &mut alter(build)),
        vec!["alter index \"foo\" rename to \"bar\""]
    );
}

#[test]
fn dropping_indexes() {
    let build = |t: &mut Blueprint| {
        t.drop_unique(vec!["email"]);
        t.drop_index("users_name_index");
        t.drop_foreign(vec!["team_id"]);
        t.drop_primary("users_pkey");
    };
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec![
            "alter table `users` drop index `users_email_unique`",
            "alter table `users` drop index `users_name_index`",
            "alter table `users` drop foreign key `users_team_id_foreign`",
            "alter table `users` drop primary key",
        ]
    );
    assert_eq!(
        sql(&pgsql(), &mut alter(build)),
        vec![
            "alter table \"users\" drop constraint \"users_email_unique\"",
            "drop index \"users_name_index\"",
            "alter table \"users\" drop constraint \"users_team_id_foreign\"",
            "alter table \"users\" drop constraint \"users_pkey\"",
        ]
    );

    let build = |t: &mut Blueprint| {
        t.drop_unique("users_email_unique");
        t.drop_index(vec!["name"]);
    };
    assert_eq!(
        sql(&sqlite(), &mut alter(build)),
        vec![
            "drop index \"users_email_unique\"",
            "drop index \"users_name_index\""
        ]
    );
}

#[test]
fn morphs() {
    let build = |t: &mut Blueprint| t.morphs("taggable");
    assert_eq!(
        sql(&mysql(), &mut create(build)),
        vec![
            "create table `users` (`taggable_type` varchar(255) not null, `taggable_id` bigint unsigned not null)",
            "alter table `users` add index `users_taggable_type_taggable_id_index`(`taggable_type`, `taggable_id`)",
        ]
    );
    let build = |t: &mut Blueprint| t.nullable_uuid_morphs("owner");
    assert_eq!(
        sql(&pgsql(), &mut create(build)),
        vec![
            "create table \"users\" (\"owner_type\" varchar(255) null, \"owner_id\" uuid null)",
            "create index \"users_owner_type_owner_id_index\" on \"users\" (\"owner_type\", \"owner_id\")",
        ]
    );
}

#[test]
fn fulltext_indexes() {
    let build = |t: &mut Blueprint| {
        t.fulltext("body");
    };
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec!["alter table `users` add fulltext `users_body_fulltext`(`body`)"]
    );
    assert_eq!(
        sql(&pgsql(), &mut alter(build)),
        vec![
            "create index \"users_body_fulltext\" on \"users\" using gin ((to_tsvector('english', \"body\")))"
        ]
    );
    assert!(alter(build).to_sql(&sqlite()).is_err());
}

#[test]
fn table_comments_and_starting_values() {
    let mut blueprint = create(|t| {
        t.id().starting_value(1000);
    });
    blueprint.comment("The users");
    assert_eq!(
        sql(&mysql(), &mut blueprint),
        vec![
            "create table `users` (`id` bigint unsigned not null auto_increment primary key)",
            "alter table `users` comment = 'The users'",
            "alter table `users` auto_increment = 1000",
        ]
    );

    let mut blueprint = create(|t| {
        t.id().starting_value(1000);
    });
    blueprint.comment("The users");
    assert_eq!(
        sql(&pgsql(), &mut blueprint),
        vec![
            "create table \"users\" (\"id\" bigserial not null primary key)",
            "comment on table \"users\" is 'The users'",
            "select setval(pg_get_serial_sequence('\"users\"', 'id'), 1000, false)",
        ]
    );
}

#[test]
fn changing_columns() {
    let build = |t: &mut Blueprint| {
        t.string_len("name", 50).nullable().change();
    };
    assert_eq!(
        sql(&mysql(), &mut alter(build)),
        vec!["alter table `users` modify `name` varchar(50) null"]
    );
    assert_eq!(
        sql(&pgsql(), &mut alter(build)),
        vec![
            "alter table \"users\" alter column \"name\" type varchar(50), alter column \"name\" drop not null, alter column \"name\" drop default, alter column \"name\" drop identity if exists",
            "comment on column \"users\".\"name\" is NULL",
        ]
    );
}

#[test]
fn sqlite_alter_commands_require_the_table_state() {
    let mut blueprint = alter(|t| {
        t.foreign("user_id").references("id").on("users");
    });
    assert!(blueprint.to_sql(&sqlite()).is_err());
}

#[test]
fn table_prefixes_apply_to_tables_and_indexes() {
    let grammar = SchemaGrammar::new(Driver::MySql, "prefix_", json!({"prefix_indexes": true}));
    let mut blueprint = Blueprint::with_prefix("users", "prefix_");
    blueprint.create();
    blueprint.string("email").unique();
    assert_eq!(
        sql(&grammar, &mut blueprint),
        vec![
            "create table `prefix_users` (`email` varchar(255) not null)",
            "alter table `prefix_users` add unique `prefix_users_email_unique`(`email`)",
        ]
    );
}

#[test]
fn mariadb_uses_native_uuids() {
    let grammar = SchemaGrammar::new(Driver::MariaDb, "", Value::Null);
    let mut blueprint = create(|t| {
        t.uuid("id").primary();
    });
    assert_eq!(
        sql(&grammar, &mut blueprint),
        vec!["create table `users` (`id` uuid not null, primary key (`id`))"]
    );
}

#[test]
fn raw_columns_and_indexes() {
    let mut blueprint = create(|t| {
        t.raw_column("point", "geometry");
        t.raw_index("lower(email)", "users_lower_email_index");
    });
    assert_eq!(
        sql(&pgsql(), &mut blueprint),
        vec![
            "create table \"users\" (\"point\" geometry not null)",
            "create index \"users_lower_email_index\" on \"users\" (lower(email))",
        ]
    );
}

#[test]
fn introspection_queries() {
    assert_eq!(
        sqlite().compile_table_exists(None, "users"),
        "select exists (select 1 from \"main\".sqlite_master where name = 'users' and type = 'table') as \"exists\""
    );
    assert_eq!(
        mysql().compile_table_exists(None, "users"),
        "select exists (select 1 from information_schema.tables where table_schema = schema() and table_name = 'users' and table_type in ('BASE TABLE', 'SYSTEM VERSIONED')) as `exists`"
    );
    assert_eq!(
        sqlite().compile_enable_foreign_key_constraints(),
        "pragma foreign_keys = 1"
    );
    assert_eq!(
        mysql().compile_disable_foreign_key_constraints(),
        "SET FOREIGN_KEY_CHECKS=0;"
    );
    assert_eq!(
        pgsql().compile_disable_foreign_key_constraints(),
        "SET CONSTRAINTS ALL DEFERRED;"
    );
    assert_eq!(
        pgsql().compile_drop_all_tables(&["public.users".into(), "posts".into()]),
        "drop table \"public\".\"users\", \"posts\" cascade"
    );
    assert_eq!(
        mysql().compile_drop_all_tables(&["users".into()]),
        "drop table `users`"
    );
}
