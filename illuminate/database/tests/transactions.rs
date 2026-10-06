//! Transactions: closures, nesting with savepoints, manual control, and
//! routing nested facade calls onto the transaction's connection.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_database::{Connection, DB, DatabaseManager, DatabaseServiceProvider, Schema};
use illuminate_support::{Result, Value, error::RuntimeException, json};

async fn memory() -> Connection {
    let db = Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:"}),
    );
    db.statement(
        "create table users (id integer primary key autoincrement, name varchar)",
        (),
    )
    .await
    .unwrap();
    db
}

fn app(database: Value) -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({ "database": database })));
    DatabaseServiceProvider.register(&container);
    (container, guard)
}

fn fail() -> illuminate_support::Error {
    RuntimeException::new("Something went wrong.").into()
}

#[tokio::test]
async fn transactions_commit() {
    let db = memory().await;
    let id = db
        .transaction(|| async {
            assert_eq!(db.transaction_level(), 1);
            db.table("users")
                .insert_get_id(json!({"name": "Taylor"}))
                .await
        })
        .await
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(db.transaction_level(), 0);
    assert_eq!(db.table("users").count().await.unwrap(), 1);
}

#[tokio::test]
async fn errors_roll_transactions_back() {
    let db = memory().await;
    let error = db
        .transaction(|| async {
            db.table("users").insert(json!({"name": "Taylor"})).await?;
            Err::<(), _>(fail())
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Something went wrong.");
    assert_eq!(db.table("users").count().await.unwrap(), 0);
    assert_eq!(db.transaction_level(), 0);
}

#[tokio::test]
async fn nested_transactions_use_savepoints() {
    let db = memory().await;
    db.transaction(|| async {
        db.table("users").insert(json!({"name": "Outer"})).await?;

        let inner = db
            .transaction(|| async {
                assert_eq!(db.transaction_level(), 2);
                db.table("users").insert(json!({"name": "Inner"})).await?;
                Err::<(), _>(fail())
            })
            .await;
        assert!(inner.is_err());
        assert_eq!(db.transaction_level(), 1);

        db.transaction(|| async {
            db.table("users")
                .insert(json!({"name": "Committed inner"}))
                .await?;
            Ok(())
        })
        .await?;

        Ok(())
    })
    .await
    .unwrap();

    let names = db
        .table("users")
        .order_by("id", "asc")
        .pluck("name")
        .await
        .unwrap();
    assert_eq!(names.all(), &[json!("Outer"), json!("Committed inner")]);
}

#[tokio::test]
async fn an_inner_failure_can_roll_back_everything() {
    let db = memory().await;
    let result = db
        .transaction(|| async {
            db.table("users").insert(json!({"name": "Outer"})).await?;
            db.transaction(|| async {
                db.table("users").insert(json!({"name": "Inner"})).await?;
                Err::<(), _>(fail())
            })
            .await
        })
        .await;
    assert!(result.is_err());
    assert_eq!(db.table("users").count().await.unwrap(), 0);
}

#[tokio::test]
async fn manual_transactions() {
    let db = memory().await;

    db.begin_transaction().await.unwrap();
    assert_eq!(db.transaction_level(), 1);
    db.table("users")
        .insert(json!({"name": "Rolled back"}))
        .await
        .unwrap();
    db.rollback().await.unwrap();
    assert_eq!(db.transaction_level(), 0);
    assert_eq!(db.table("users").count().await.unwrap(), 0);

    db.begin_transaction().await.unwrap();
    db.table("users")
        .insert(json!({"name": "Kept"}))
        .await
        .unwrap();
    db.begin_transaction().await.unwrap();
    assert_eq!(db.transaction_level(), 2);
    db.table("users")
        .insert(json!({"name": "Savepoint"}))
        .await
        .unwrap();
    db.rollback().await.unwrap();
    assert_eq!(db.transaction_level(), 1);
    db.commit().await.unwrap();
    assert_eq!(db.transaction_level(), 0);

    let names = db.table("users").pluck("name").await.unwrap();
    assert_eq!(names.all(), &[json!("Kept")]);

    // Committing or rolling back without a transaction is harmless.
    db.commit().await.unwrap();
    db.rollback().await.unwrap();
}

#[tokio::test]
async fn closures_nest_inside_manual_transactions() {
    let db = memory().await;
    db.begin_transaction().await.unwrap();
    db.transaction(|| async {
        assert_eq!(db.transaction_level(), 2);
        db.table("users").insert(json!({"name": "Nested"})).await?;
        Ok(())
    })
    .await
    .unwrap();
    db.rollback().await.unwrap();
    assert_eq!(db.table("users").count().await.unwrap(), 0);
}

#[tokio::test]
async fn after_commit_callbacks() {
    let db = memory().await;
    let calls = Arc::new(Mutex::new(Vec::new()));

    let log = calls.clone();
    db.after_commit(move || log.lock().unwrap().push("immediately"));

    db.transaction(|| async {
        let log = calls.clone();
        db.after_commit(move || log.lock().unwrap().push("outer"));
        let _ = db
            .transaction(|| async {
                let log = calls.clone();
                db.after_commit(move || log.lock().unwrap().push("discarded"));
                Err::<(), _>(fail())
            })
            .await;
        db.transaction(|| async {
            let log = calls.clone();
            db.after_commit(move || log.lock().unwrap().push("inner"));
            Ok(())
        })
        .await?;
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "callbacks wait for the outermost commit"
        );
        Ok(())
    })
    .await
    .unwrap();

    assert_eq!(
        *calls.lock().unwrap(),
        vec!["immediately", "outer", "inner"]
    );

    let log = calls.clone();
    let _ = db
        .transaction(|| async {
            db.after_commit(move || log.lock().unwrap().push("rolled back"));
            Err::<(), _>(fail())
        })
        .await;
    assert_eq!(calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn cancelled_transactions_are_rolled_back() {
    let db = memory().await;
    let pending = db.transaction(|| async {
        db.table("users")
            .insert(json!({"name": "Abandoned"}))
            .await?;
        std::future::pending::<()>().await;
        Ok(())
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), pending)
            .await
            .is_err()
    );

    // The connection went back to the pool and the insert was rolled back.
    assert_eq!(db.table("users").count().await.unwrap(), 0);
    assert_eq!(db.transaction_level(), 0);
}

#[tokio::test]
async fn transactions_retry_only_on_deadlocks() {
    let db = memory().await;
    let attempts = AtomicUsize::new(0);
    let result = db
        .transaction_with_attempts(3, || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(fail())
        })
        .await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    let attempts = AtomicUsize::new(0);
    let result = db
        .transaction_with_attempts(3, || async {
            if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                return Err(RuntimeException::new(
                    "SQLSTATE[40001]: Deadlock found when trying to get lock",
                )
                .into());
            }
            db.table("users")
                .insert(json!({"name": "Third time"}))
                .await?;
            Ok(())
        })
        .await;
    assert!(result.is_ok());
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(db.table("users").count().await.unwrap(), 1);
}

#[tokio::test]
async fn sqlite_transaction_modes_are_configurable() {
    let db = Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": ":memory:", "transaction_mode": "IMMEDIATE"}),
    );
    db.statement("create table users (name varchar)", ())
        .await
        .unwrap();
    db.transaction(|| async {
        db.table("users").insert(json!({"name": "Taylor"})).await?;
        db.transaction(|| async { db.table("users").insert(json!({"name": "Nested"})).await })
            .await?;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(db.table("users").count().await.unwrap(), 2);
}

#[tokio::test]
async fn connections_without_a_driver_fail() {
    let db = Connection::new("broken", json!({"database": ":memory:"}));
    let error = db.select("select 1", ()).await.unwrap_err();
    assert!(error.to_string().starts_with("A driver must be specified."));
}

#[tokio::test]
async fn other_tasks_do_not_see_uncommitted_work() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("transactions.sqlite");
    let db = Connection::new(
        "sqlite",
        json!({"driver": "sqlite", "database": path.to_string_lossy()}),
    );
    db.statement(
        "create table users (id integer primary key autoincrement, name varchar)",
        (),
    )
    .await
    .unwrap();

    db.transaction(|| async {
        db.table("users")
            .insert(json!({"name": "Uncommitted"}))
            .await?;
        assert_eq!(
            db.table("users").count().await?,
            1,
            "the transaction sees its own writes"
        );

        // A separate task gets its own connection from the pool.
        let other = db.clone();
        let seen = tokio::spawn(async move { other.table("users").count().await.unwrap() })
            .await
            .unwrap();
        assert_eq!(seen, 0);
        Ok(())
    })
    .await
    .unwrap();

    assert_eq!(db.table("users").count().await.unwrap(), 1);
}

#[tokio::test]
async fn facade_calls_inside_a_transaction_use_its_connection() {
    let (_container, _guard) = app(json!({
        "default": "sqlite",
        "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
    }));

    Schema::create("users", |table| {
        table.id();
        table.string("name");
    })
    .await
    .unwrap();

    // The in-memory pool holds a single connection: if nested facade calls
    // didn't reuse the transaction's connection, this would dead-lock.
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        DB::transaction(|| async {
            DB::table("users").insert(json!({"name": "Taylor"})).await?;
            DB::insert("insert into users (name) values (?)", ("Abigail",)).await?;
            assert_eq!(DB::transaction_level(), 1);
            Schema::table("users", |table| {
                table.integer("votes").default(0);
            })
            .await?;
            DB::transaction(|| async {
                assert_eq!(DB::transaction_level(), 2);
                DB::table("users").update(json!({"votes": 1})).await?;
                Ok(())
            })
            .await?;
            DB::table("users").count().await
        }),
    )
    .await
    .expect("nested calls must not wait for another pool connection")
    .unwrap();

    assert_eq!(result, 2);
    assert_eq!(DB::table("users").sum("votes").await.unwrap(), json!(2));

    let failed: Result<()> = DB::transaction(|| async {
        DB::table("users").delete().await?;
        Err(fail())
    })
    .await;
    assert!(failed.is_err());
    assert_eq!(DB::table("users").count().await.unwrap(), 2);

    DB::begin_transaction().await.unwrap();
    DB::table("users").delete().await.unwrap();
    DB::rollback().await.unwrap();
    assert_eq!(DB::table("users").count().await.unwrap(), 2);
}

#[tokio::test]
async fn the_manager_resolves_and_caches_connections() {
    let (container, _guard) = app(json!({
        "default": "main",
        "connections": {
            "main": {"driver": "sqlite", "database": ":memory:"},
            "other": {"driver": "sqlite", "database": ":memory:", "prefix": "x_"},
            "pgsql": {"driver": "pgsql", "host": "127.0.0.1"},
        },
    }));

    let manager = container.make::<DatabaseManager>();
    assert_eq!(manager.get_default_connection(), "main");
    assert!(DB::connection("main").same_as(&DB::default_connection()));
    assert!(!DB::connection("main").same_as(&DB::connection("other")));
    assert_eq!(DB::connection("other").get_table_prefix(), "x_");
    assert_eq!(DB::connection("pgsql").get_driver_name(), "pgsql");
    assert_eq!(DB::connection("main").get_config("name"), json!("main"));

    DB::statement("create table t (id integer)", ())
        .await
        .unwrap();
    DB::table("t").insert(json!({"id": 1})).await.unwrap();

    // Each connection is its own database.
    assert!(DB::connection("other").table("t").count().await.is_err());

    // The default can be switched globally, or for the current task only.
    let count = manager
        .using_connection("other", async { DB::get_default_connection() })
        .await;
    assert_eq!(count, "other");
    assert_eq!(DB::get_default_connection(), "main");
    DB::set_default_connection("other");
    assert_eq!(DB::get_default_connection(), "other");
    DB::set_default_connection("main");

    // Purging forgets the connection; an in-memory database starts over.
    DB::purge("main").await;
    assert!(DB::table("t").count().await.is_err());

    let events = Arc::new(AtomicUsize::new(0));
    let counter = events.clone();
    DB::listen(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    DB::select("select 1", ()).await.unwrap();
    DB::connection("other")
        .select("select 1", ())
        .await
        .unwrap();
    assert_eq!(events.load(Ordering::SeqCst), 2);

    DB::enable_query_log();
    DB::select("select ?", (5,)).await.unwrap();
    assert_eq!(DB::get_query_log().len(), 1);
    DB::flush_query_log();
    assert!(DB::get_query_log().is_empty());

    let missing = DB::connection("missing")
        .select("select 1", ())
        .await
        .unwrap_err();
    assert!(
        missing
            .to_string()
            .starts_with("Database connection [missing] not configured.")
    );
}

#[tokio::test]
async fn the_connection_binding_resolves_the_default_connection() {
    let (container, _guard) = app(json!({
        "default": "sqlite",
        "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
    }));
    let connection = container.make::<Connection>();
    assert_eq!(connection.get_name(), "sqlite");
    assert!(connection.same_as(&DB::default_connection()));
}

fn assert_send<T: Send>(_: T) {}

#[test]
fn database_futures_are_send() {
    let db = Connection::new("sqlite", json!({"driver": "sqlite"}));
    assert_send(db.table("users").get());
    assert_send(db.table("users").first_as::<Value>());
    assert_send(db.table("users").insert(json!({"a": 1})));
    assert_send(db.table("users").update(json!({"a": 1})));
    assert_send(db.table("users").chunk(10, |_, _| async { Ok(true) }));
    assert_send(db.select("select 1", ()));
    assert_send(db.transaction(|| async { Ok(()) }));
    assert_send(db.begin_transaction());
    assert_send(DB::transaction(|| async { Ok(()) }));
    assert_send(Schema::create("users", |t| {
        t.id();
    }));
    assert_send(Schema::has_table("users"));
}
