//! Vector, spatial, computed and tsvector columns, vector / spatial / full
//! text indexes, `foreign_id_for`, column modifiers and the schema
//! builder's conditional helpers.

use illuminate_database::Connection;
use illuminate_database::Driver;
use illuminate_database::eloquent::*;
use illuminate_database::schema::{Blueprint, Schema, SchemaGrammar};
use illuminate_support::error::{LogicException, RuntimeException};

fn sqlite() -> SchemaGrammar {
    SchemaGrammar::new(Driver::Sqlite, "", Value::Null)
}

fn mysql() -> SchemaGrammar {
    SchemaGrammar::new(Driver::MySql, "", Value::Null)
}

fn mariadb() -> SchemaGrammar {
    SchemaGrammar::new(Driver::MariaDb, "", Value::Null)
}

fn pgsql() -> SchemaGrammar {
    SchemaGrammar::new(Driver::Postgres, "", Value::Null)
}

fn alter(grammar: &SchemaGrammar, build: impl Fn(&mut Blueprint)) -> Vec<String> {
    let mut blueprint = Blueprint::new("users");
    build(&mut blueprint);
    blueprint.to_sql(grammar).unwrap()
}

fn alter_error(grammar: &SchemaGrammar, build: impl Fn(&mut Blueprint)) -> String {
    let mut blueprint = Blueprint::new("users");
    build(&mut blueprint);
    blueprint.to_sql(grammar).unwrap_err().to_string()
}

#[derive(Debug, Clone, Default, Model)]
#[table("writers")]
pub struct Author {
    pub id: u64,
}

#[derive(Debug, Clone, Default, Model)]
#[has_uuids]
pub struct Token {
    pub id: String,
}

#[derive(Debug, Clone, Default, Model)]
#[has_ulids]
pub struct Ticket {
    pub id: String,
}

#[test]
fn vector_columns_and_indexes() {
    assert_eq!(
        alter(&pgsql(), |t| {
            t.vector("embedding", Some(1536)).vector_index();
        }),
        vec![
            "alter table \"users\" add column \"embedding\" vector(1536) not null",
            "create index \"users_embedding_vectorindex\" on \"users\" using hnsw (\"embedding\" vector_cosine_ops)",
        ]
    );
    assert_eq!(
        alter(&mariadb(), |t| {
            t.vector("embedding", Some(3));
            t.vector_index_named("embedding", "emb");
        }),
        vec![
            "alter table `users` add `embedding` vector(3) not null",
            "alter table `users` add vector index `emb`(`embedding`) M=6 DISTANCE=cosine",
        ]
    );
    assert_eq!(
        alter(&mysql(), |t| {
            t.vector("embedding", None);
        }),
        vec!["alter table `users` add `embedding` vector not null"]
    );
    assert_eq!(
        alter_error(&sqlite(), |t| {
            t.vector("embedding", Some(3));
        }),
        "This database driver does not support the vector type."
    );
    assert_eq!(
        alter_error(&mysql(), |t| {
            t.vector_index("embedding");
        }),
        "The database driver in use does not support vector indexes."
    );
    assert_eq!(
        alter(&pgsql(), |t| t.drop_vector_index("emb")),
        vec!["drop index \"emb\""]
    );
    assert_eq!(
        alter(&mariadb(), |t| t.drop_vector_index(vec!["embedding"])),
        vec!["alter table `users` drop index `users_embedding_vectorindex`"]
    );
    assert_eq!(
        alter_error(&sqlite(), |t| t.drop_vector_index("emb")),
        "The database driver in use does not support vector indexes."
    );
}

#[test]
fn spatial_columns_and_indexes() {
    assert_eq!(
        alter(&mysql(), |t| {
            t.geometry_with("location", Some("Point"), 4326);
            t.geography("area");
            t.geometry("shape").spatial_index();
        }),
        vec![
            "alter table `users` add `location` point srid 4326 not null",
            "alter table `users` add `area` geometry srid 4326 not null",
            "alter table `users` add `shape` geometry not null",
            "alter table `users` add spatial index `users_shape_spatialindex`(`shape`)",
        ]
    );
    assert_eq!(
        alter(&mariadb(), |t| {
            t.geometry_with("location", Some("point"), 4326);
        }),
        vec!["alter table `users` add `location` point ref_system_id=4326 not null"]
    );
    assert_eq!(
        alter(&pgsql(), |t| {
            t.geometry_with("location", Some("Point"), 4326);
            t.geography("area");
            t.geography_with("path", Some("linestring"), 0);
            t.spatial_index("location");
            t.spatial_index(["area"])
                .operator_class("gist_geography_ops");
        }),
        vec![
            "alter table \"users\" add column \"location\" geometry(point,4326) not null",
            "alter table \"users\" add column \"area\" geography not null",
            "alter table \"users\" add column \"path\" geography(linestring) not null",
            "create index \"users_location_spatialindex\" on \"users\" using gist (\"location\")",
            "create index \"users_area_spatialindex\" on \"users\" using gist (\"area\" gist_geography_ops)",
        ]
    );
    assert_eq!(
        alter(&sqlite(), |t| {
            t.geometry("location").nullable();
        }),
        vec!["alter table \"users\" add column \"location\" geometry"]
    );
    assert_eq!(
        alter_error(&sqlite(), |t| {
            t.spatial_index("location");
        }),
        "The database driver in use does not support spatial indexes."
    );
    assert_eq!(
        alter(&mysql(), |t| t.drop_spatial_index(vec!["location"])),
        vec!["alter table `users` drop index `users_location_spatialindex`"]
    );
    assert_eq!(
        alter(&pgsql(), |t| t.drop_spatial_index("geo")),
        vec!["drop index \"geo\""]
    );
}

#[test]
fn full_text_aliases_tsvector_and_computed_columns() {
    assert_eq!(
        alter(&mysql(), |t| {
            t.full_text(["title", "body"]);
            t.drop_full_text("posts_title_fulltext");
        }),
        vec![
            "alter table `users` add fulltext `users_title_body_fulltext`(`title`, `body`)",
            "alter table `users` drop index `posts_title_fulltext`",
        ]
    );
    assert_eq!(
        alter(&pgsql(), |t| {
            t.tsvector("search").nullable();
        }),
        vec!["alter table \"users\" add column \"search\" tsvector null"]
    );
    assert_eq!(
        alter_error(&mysql(), |t| {
            t.tsvector("search");
        }),
        "This database driver does not support the tsvector type."
    );
    assert_eq!(
        alter_error(&pgsql(), |t| {
            t.computed("total", "price * quantity").persisted();
        }),
        "This database driver does not support the computed type."
    );
    assert_eq!(
        alter_error(&sqlite(), |t| {
            t.computed("total", "price * quantity");
        }),
        "This database driver requires a type, see the virtualAs / storedAs modifiers."
    );
    let mut table = Blueprint::creating("orders");
    table.computed("total", "price * quantity");
    assert!(table.to_sql(&mysql()).is_err());
}

#[test]
fn column_modifiers() {
    assert_eq!(
        alter(&mysql(), |t| {
            t.string("name").instant();
            t.string("email").lock("none");
            t.integer("votes").type_("bigInteger").change().instant();
        }),
        vec![
            "alter table `users` add `name` varchar(255) not null, algorithm=instant",
            "alter table `users` add `email` varchar(255) not null, lock=none",
            "alter table `users` modify `votes` bigint not null, algorithm=instant",
        ]
    );
    assert_eq!(
        alter(&pgsql(), |t| {
            t.string("name").instant();
        }),
        vec!["alter table \"users\" add column \"name\" varchar(255) not null"]
    );
}

#[test]
fn timestamps_and_soft_deletes_helpers() {
    assert_eq!(
        alter(&mysql(), |t| {
            t.nullable_timestamps_tz();
            t.soft_deletes_datetime();
            t.soft_deletes_datetime_named("archived_at");
        }),
        vec![
            "alter table `users` add `created_at` timestamp null",
            "alter table `users` add `updated_at` timestamp null",
            "alter table `users` add `deleted_at` datetime null",
            "alter table `users` add `archived_at` datetime null",
        ]
    );
    assert_eq!(
        alter(&mysql(), |t| t.drop_soft_deletes_tz()),
        vec!["alter table `users` drop `deleted_at`"]
    );
}

#[test]
fn foreign_ids_for_models() {
    assert_eq!(
        alter(&mysql(), |t| {
            t.foreign_id_for::<Author>()
                .constrained()
                .cascade_on_delete();
            t.foreign_id_for::<Token>();
            t.foreign_id_for::<Ticket>();
            t.foreign_uuid_for::<Author>();
            t.foreign_ulid_for::<Token>();
            t.foreign_id_for_column::<Author>("writer_id").constrained();
        }),
        vec![
            "alter table `users` add `author_id` bigint unsigned not null",
            "alter table `users` add constraint `users_author_id_foreign` foreign key (`author_id`) references `writers` (`id`) on delete cascade",
            "alter table `users` add `token_id` char(36) not null",
            "alter table `users` add `ticket_id` char(26) not null",
            "alter table `users` add `author_id` char(36) not null",
            "alter table `users` add `token_id` char(26) not null",
            "alter table `users` add `writer_id` bigint unsigned not null",
            "alter table `users` add constraint `users_writer_id_foreign` foreign key (`writer_id`) references `writers` (`id`)",
        ]
    );
    assert_eq!(
        alter(&mysql(), |t| t.drop_foreign_id_for::<Author>()),
        vec!["alter table `users` drop `author_id`"]
    );
    assert_eq!(
        alter(&mysql(), |t| t.drop_constrained_foreign_id_for::<Author>()),
        vec![
            "alter table `users` drop foreign key `users_author_id_foreign`",
            "alter table `users` drop `author_id`",
        ]
    );
}

#[test]
fn the_default_time_precision_is_configurable() {
    let precision = |grammar: &SchemaGrammar| {
        alter(grammar, |t| {
            t.timestamp("published_at");
        })
    };
    assert_eq!(
        precision(&pgsql()),
        vec![
            "alter table \"users\" add column \"published_at\" timestamp(0) without time zone not null"
        ]
    );
    Schema::default_time_precision(Some(6));
    let six = precision(&mysql());
    Schema::default_time_precision(None);
    let none = precision(&pgsql());
    Schema::default_time_precision(Some(0));
    assert_eq!(
        six,
        vec!["alter table `users` add `published_at` timestamp(6) not null"]
    );
    assert_eq!(
        none,
        vec![
            "alter table \"users\" add column \"published_at\" timestamp without time zone not null"
        ]
    );
}

#[test]
fn extensions_and_types_compile_for_postgres() {
    let grammar = pgsql();
    assert_eq!(
        grammar.compile_create_extension("vector", None),
        "create extension if not exists \"vector\""
    );
    assert_eq!(
        grammar.compile_create_extension("postgis", Some("public")),
        "create extension if not exists \"postgis\" schema \"public\""
    );
    assert_eq!(
        grammar.compile_drop_all_types(&["public.mood".into(), "public.color".into()]),
        "drop type \"public\".\"mood\", \"public\".\"color\" cascade"
    );
    assert_eq!(
        grammar.compile_drop_all_domains(&["public.email".into()]),
        "drop domain \"public\".\"email\" cascade"
    );
    assert!(grammar.compile_types().contains("current_schema()"));
}

#[tokio::test]
async fn conditional_schema_changes_on_sqlite() {
    let db = Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:"}),
    );
    let schema = db.get_schema_builder();
    schema
        .create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
        })
        .await
        .unwrap();

    schema
        .when_table_has_column("users", "name", |table| {
            table.string("nickname").nullable();
        })
        .await
        .unwrap();
    schema
        .when_table_has_column("users", "missing", |table| {
            table.string("never").nullable();
        })
        .await
        .unwrap();
    schema
        .when_table_doesnt_have_column("users", "bio", |table| {
            table.text("bio").nullable();
        })
        .await
        .unwrap();
    schema
        .when_table_doesnt_have_column("users", "name", |table| {
            table.string("never_either").nullable();
        })
        .await
        .unwrap();
    assert!(schema.has_column("users", "nickname").await.unwrap());
    assert!(schema.has_column("users", "bio").await.unwrap());
    assert!(!schema.has_column("users", "never").await.unwrap());
    assert!(!schema.has_column("users", "never_either").await.unwrap());

    schema
        .when_table_has_index("users", vec!["email"], Some("unique"), |table| {
            table.drop_unique(vec!["email"]);
        })
        .await
        .unwrap();
    assert!(
        !schema
            .has_index("users", vec!["email"], None)
            .await
            .unwrap()
    );
    schema
        .when_table_doesnt_have_index("users", "users_name_index", None, |table| {
            table.index("name");
        })
        .await
        .unwrap();
    assert!(
        schema
            .has_index("users", "users_name_index", None)
            .await
            .unwrap()
    );

    let error = schema
        .ensure_vector_extension_exists(None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Extensions are only supported by Postgres."
    );
    assert!(error.downcast_ref::<RuntimeException>().is_some());
    let error = schema.drop_all_types().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "This database driver does not support dropping all types."
    );
    assert!(error.downcast_ref::<LogicException>().is_some());
}
