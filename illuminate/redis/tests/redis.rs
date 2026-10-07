//! The Redis component against a real `redis-server`, started for this test
//! binary. Every test works in a database of its own.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_redis::testing::RedisServer;
use illuminate_redis::{
    ConcurrencyLimiter, Connection, ConnectionConfig, DurationLimiter, LimiterTimeoutException,
    Redis, RedisManager, RedisServiceProvider,
};
use illuminate_support::{Value, json};

/// A connection to a fresh database of the shared server.
fn connection(server: &RedisServer, prefix: &str) -> Connection {
    let mut config = server.connection_config();
    config["prefix"] = json!(prefix);
    Connection::new(
        "default",
        ConnectionConfig::parse(&config, &json!({})).unwrap(),
    )
    .unwrap()
}

/// A second, unprefixed connection to the same database.
fn raw(connection: &Connection) -> Connection {
    let config = ConnectionConfig {
        prefix: String::new(),
        ..connection.config().clone()
    };
    Connection::new("raw", config).unwrap()
}

/// An application whose `database.redis` points at the shared server.
fn app(server: &RedisServer, options: Value) -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    let mut redis = server.config();
    redis["options"] = options;
    container.instance(Repository::new(json!({
        "app": {"name": "Laravel"},
        "database": {"redis": redis},
    })));
    RedisServiceProvider.register(&container);
    (container, guard)
}

// ----------------------------------------------------------------------
// Strings & keys
// ----------------------------------------------------------------------

#[tokio::test]
async fn strings_and_keys() {
    let server = RedisServer::shared();
    let redis = connection(&server, "");

    assert!(redis.set("name", "Taylor").await.unwrap());
    assert_eq!(redis.get("name").await.unwrap().as_deref(), Some("Taylor"));
    assert_eq!(redis.get("missing").await.unwrap(), None);

    assert!(redis.setnx("lock", "a").await.unwrap());
    assert!(!redis.setnx("lock", "b").await.unwrap());
    assert_eq!(redis.get("lock").await.unwrap().as_deref(), Some("a"));

    assert!(redis.set_ex("token", "abc", 100).await.unwrap());
    let ttl = redis.ttl("token").await.unwrap();
    assert!((99..=100).contains(&ttl), "{ttl}");
    assert_eq!(redis.ttl("name").await.unwrap(), -1);
    assert_eq!(redis.ttl("missing").await.unwrap(), -2);
    assert!(redis.expire("name", 50).await.unwrap());
    assert!(!redis.expire("missing", 50).await.unwrap());

    assert_eq!(redis.incr("visits").await.unwrap(), 1);
    assert_eq!(redis.incrby("visits", 10).await.unwrap(), 11);
    assert_eq!(redis.decr("visits").await.unwrap(), 10);
    assert_eq!(redis.decrby("visits", 4).await.unwrap(), 6);
    assert!(redis.incr("name").await.is_err(), "not an integer");

    assert_eq!(redis.exists("name").await.unwrap(), 1);
    assert_eq!(
        redis.exists(["name", "visits", "missing"]).await.unwrap(),
        2
    );

    assert!(redis.mset([("a", 1), ("b", 2)]).await.unwrap());
    assert!(redis.mset(Vec::<(&str, i32)>::new()).await.unwrap());
    assert_eq!(
        redis.mget(["a", "missing", "b"]).await.unwrap(),
        [Some("1".to_string()), None, Some("2".to_string())]
    );
    assert!(redis.mget(Vec::<String>::new()).await.unwrap().is_empty());

    let mut keys = redis.keys("*").await.unwrap();
    keys.sort();
    assert_eq!(keys, ["a", "b", "lock", "name", "token", "visits"]);

    assert_eq!(redis.del("a").await.unwrap(), 1);
    assert_eq!(redis.del(["b", "lock", "missing"]).await.unwrap(), 2);

    assert!(redis.flushdb().await.unwrap());
    assert!(redis.keys("*").await.unwrap().is_empty());
}

#[tokio::test]
async fn keys_are_prefixed_like_phpredis_does() {
    let server = RedisServer::shared();
    let redis = connection(&server, "laravel-database-");
    let raw = raw(&redis);

    redis.set("name", "Taylor").await.unwrap();
    redis.mset([("first", "a"), ("second", "b")]).await.unwrap();
    redis.rpush("names", ["Taylor", "Abigail"]).await.unwrap();

    // The keys are stored with the prefix, the values are not...
    assert_eq!(
        raw.get("laravel-database-name").await.unwrap().as_deref(),
        Some("Taylor")
    );
    assert_eq!(raw.get("name").await.unwrap(), None);
    assert_eq!(
        raw.mget(["laravel-database-first", "laravel-database-second"])
            .await
            .unwrap(),
        [Some("a".to_string()), Some("b".to_string())]
    );
    assert_eq!(
        raw.lrange("laravel-database-names", 0, -1).await.unwrap(),
        ["Taylor", "Abigail"]
    );

    // ...and the prefix is transparent when reading them back.
    assert_eq!(redis.get("name").await.unwrap().as_deref(), Some("Taylor"));
    let mut keys = redis.keys("*").await.unwrap();
    keys.sort();
    assert_eq!(keys, ["first", "name", "names", "second"]);

    // Raw commands are prefixed too, unless they are executed raw.
    assert_eq!(redis.command("get", "name").await.unwrap(), json!("Taylor"));
    assert_eq!(
        redis.execute_raw(("GET", "name")).await.unwrap(),
        Value::Null
    );
    assert_eq!(
        redis
            .execute_raw(("GET", "laravel-database-name"))
            .await
            .unwrap(),
        json!("Taylor")
    );
    assert_eq!(redis.execute_raw(()).await.unwrap(), Value::Null);
    assert_eq!(
        redis
            .command("keys", "*")
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn any_command_can_be_run() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");

    redis.rpush("names", ["a", "b", "c", "d"]).await.unwrap();
    assert_eq!(
        redis.command("lrange", ("names", 1, 2)).await.unwrap(),
        json!(["b", "c"])
    );
    assert_eq!(
        redis
            .command("lrange", json!(["names", 0, 0]))
            .await
            .unwrap(),
        json!(["a"])
    );
    assert_eq!(redis.command("ping", ()).await.unwrap(), json!("PONG"));
    assert_eq!(
        redis.command("set", ("name", "Taylor")).await.unwrap(),
        json!(true)
    );
    assert_eq!(
        redis
            .command("set", ("name", "Abigail", "NX"))
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(redis.command("llen", "names").await.unwrap(), json!(4));
    assert_eq!(
        redis
            .command("hset", json!(["user", {"name": "Taylor", "role": "admin"}]))
            .await
            .unwrap(),
        json!(2)
    );

    let bytes: Option<Vec<u8>> = redis.query("get", "name").await.unwrap();
    assert_eq!(bytes.as_deref(), Some(&b"Taylor"[..]));
    redis
        .command("set", ("binary", vec![0_u8, 159, 146, 150]))
        .await
        .unwrap();
    let binary: Vec<u8> = redis.query("get", "binary").await.unwrap();
    assert_eq!(binary, [0, 159, 146, 150]);

    let error = redis.command("nonsense", ()).await.unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("unknown command"),
        "{error}"
    );
}

// ----------------------------------------------------------------------
// Data structures
// ----------------------------------------------------------------------

#[tokio::test]
async fn hashes() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");

    assert_eq!(redis.hset("user:1", "name", "Taylor").await.unwrap(), 1);
    assert_eq!(
        redis.hset("user:1", "name", "Taylor Otwell").await.unwrap(),
        0
    );
    redis.hset("user:1", "visits", 1).await.unwrap();

    assert_eq!(
        redis.hget("user:1", "name").await.unwrap().as_deref(),
        Some("Taylor Otwell")
    );
    assert_eq!(redis.hget("user:1", "missing").await.unwrap(), None);
    assert!(redis.hexists("user:1", "name").await.unwrap());
    assert!(!redis.hexists("user:1", "email").await.unwrap());
    assert_eq!(redis.hincrby("user:1", "visits", 4).await.unwrap(), 5);
    assert_eq!(redis.hlen("user:1").await.unwrap(), 2);
    assert_eq!(redis.hkeys("user:1").await.unwrap(), ["name", "visits"]);

    let all = redis.hgetall("user:1").await.unwrap();
    assert_eq!(all["name"], "Taylor Otwell");
    assert_eq!(all["visits"], "5");
    assert!(redis.hgetall("missing").await.unwrap().is_empty());

    assert_eq!(
        redis.hdel("user:1", ["visits", "missing"]).await.unwrap(),
        1
    );
    assert_eq!(redis.hdel("user:1", "name").await.unwrap(), 1);
    assert_eq!(redis.exists("user:1").await.unwrap(), 0);
}

#[tokio::test]
async fn lists() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");

    assert_eq!(redis.rpush("names", "Abigail").await.unwrap(), 1);
    assert_eq!(redis.lpush("names", "Taylor").await.unwrap(), 2);
    assert_eq!(
        redis
            .rpush("names", ["James", "Dayle", "James"])
            .await
            .unwrap(),
        5
    );
    assert_eq!(redis.llen("names").await.unwrap(), 5);
    assert_eq!(
        redis.lrange("names", 0, -1).await.unwrap(),
        ["Taylor", "Abigail", "James", "Dayle", "James"]
    );
    assert_eq!(
        redis.lrange("names", 1, 2).await.unwrap(),
        ["Abigail", "James"]
    );
    assert_eq!(
        redis.lindex("names", -1).await.unwrap().as_deref(),
        Some("James")
    );
    assert_eq!(redis.lrem("names", 0, "James").await.unwrap(), 2);
    assert_eq!(
        redis.lpop("names").await.unwrap().as_deref(),
        Some("Taylor")
    );
    assert_eq!(redis.rpop("names").await.unwrap().as_deref(), Some("Dayle"));
    assert_eq!(
        redis.lpop("names").await.unwrap().as_deref(),
        Some("Abigail")
    );
    assert_eq!(redis.lpop("names").await.unwrap(), None);
}

#[tokio::test]
async fn sets_and_sorted_sets() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");

    assert_eq!(redis.sadd("tags", ["php", "rust", "php"]).await.unwrap(), 2);
    assert_eq!(redis.sadd("tags", "laravel").await.unwrap(), 1);
    assert_eq!(redis.scard("tags").await.unwrap(), 3);
    assert!(redis.sismember("tags", "rust").await.unwrap());
    assert!(!redis.sismember("tags", "go").await.unwrap());
    assert_eq!(redis.srem("tags", ["php", "go"]).await.unwrap(), 1);
    let mut members = redis.smembers("tags").await.unwrap();
    members.sort();
    assert_eq!(members, ["laravel", "rust"]);

    assert_eq!(redis.zadd("board", 10.0, "taylor").await.unwrap(), 1);
    assert_eq!(redis.zadd("board", 5.0, "abigail").await.unwrap(), 1);
    assert_eq!(redis.zadd("board", 7.5, "james").await.unwrap(), 1);
    assert_eq!(redis.zadd("board", 1.0, "abigail").await.unwrap(), 0);
    assert_eq!(redis.zcard("board").await.unwrap(), 3);
    assert_eq!(
        redis.zrange("board", 0, -1).await.unwrap(),
        ["abigail", "james", "taylor"]
    );
    assert_eq!(
        redis.zrange_with_scores("board", 0, 1).await.unwrap(),
        [("abigail".to_string(), 1.0), ("james".to_string(), 7.5)]
    );
    assert_eq!(
        redis.zrangebyscore("board", 5, "+inf").await.unwrap(),
        ["james", "taylor"]
    );
    assert_eq!(
        redis.zrangebyscore("board", "-inf", "(7.5").await.unwrap(),
        ["abigail"]
    );
    assert_eq!(redis.zscore("board", "james").await.unwrap(), Some(7.5));
    assert_eq!(redis.zscore("board", "nobody").await.unwrap(), None);
    assert_eq!(redis.zincrby("board", 2.5, "james").await.unwrap(), 10.0);
    assert_eq!(redis.zrem("board", ["taylor", "nobody"]).await.unwrap(), 1);
    assert_eq!(redis.zcard("board").await.unwrap(), 2);
}

// ----------------------------------------------------------------------
// Pipelines, transactions & scripts
// ----------------------------------------------------------------------

#[tokio::test]
async fn pipelines_send_every_command_at_once() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");

    let replies = redis
        .pipeline(|pipe| {
            for i in 0..1000 {
                pipe.set(format!("key:{i}"), i);
            }
            pipe.get("key:999").incr("key:1");
        })
        .await
        .unwrap();
    assert_eq!(replies.len(), 1002);
    assert_eq!(replies[0], json!(true));
    assert_eq!(replies[1000], json!("999"));
    assert_eq!(replies[1001], json!(2));
    assert_eq!(redis.get("key:500").await.unwrap().as_deref(), Some("500"));
    assert_eq!(
        raw(&redis).get("app:key:500").await.unwrap().as_deref(),
        Some("500")
    );

    assert!(redis.pipeline(|_| {}).await.unwrap().is_empty());
}

#[tokio::test]
async fn transactions_are_atomic() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");

    let replies = redis
        .transaction(|redis| {
            redis.incr("user_visits");
            redis.incr("total_visits");
            redis.incrby("total_visits", 10);
        })
        .await
        .unwrap();
    assert_eq!(replies, [json!(1), json!(1), json!(11)]);

    // Concurrent transactions never interleave.
    let mut handles = Vec::new();
    for _ in 0..20 {
        let redis = redis.clone();
        handles.push(tokio::spawn(async move {
            redis
                .transaction(|tx| {
                    tx.rpush("log", "begin");
                    tx.rpush("log", "end");
                })
                .await
                .unwrap()
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    let log = redis.lrange("log", 0, -1).await.unwrap();
    assert_eq!(log.len(), 40);
    for pair in log.chunks(2) {
        assert_eq!(pair, ["begin", "end"]);
    }

    // A failing command inside the transaction fails the transaction.
    redis.set("name", "Taylor").await.unwrap();
    assert!(
        redis
            .transaction(|tx| {
                tx.incr("name");
            })
            .await
            .is_err()
    );
    assert!(redis.transaction(|_| {}).await.unwrap().is_empty());
}

#[tokio::test]
async fn lua_scripts_run_atomically() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");
    let script = r#"
        local counter = redis.call("incr", KEYS[1])

        if counter > 5 then
            redis.call("incr", KEYS[2])
        end

        return counter
    "#;

    for expected in 1..=7 {
        let value = redis
            .eval(script, ("first-counter", "second-counter"), ())
            .await
            .unwrap();
        assert_eq!(value, json!(expected));
    }
    assert_eq!(
        redis.get("second-counter").await.unwrap().as_deref(),
        Some("2")
    );
    assert_eq!(
        raw(&redis)
            .get("app:first-counter")
            .await
            .unwrap()
            .as_deref(),
        Some("7")
    );

    // Arguments are passed as ARGV, and the script cache can be flushed.
    redis.command("script", "flush").await.unwrap();
    let value = redis
        .eval("return {KEYS[1], ARGV[1], ARGV[2]}", ("key",), ("a", 2))
        .await
        .unwrap();
    assert_eq!(value, json!(["app:key", "a", "2"]));
    let value = redis.eval("return false", (), ()).await.unwrap();
    assert_eq!(value, Value::Null);
    assert!(
        redis
            .eval("return redis.call('nope')", (), ())
            .await
            .is_err()
    );
}

// ----------------------------------------------------------------------
// Blocking commands
// ----------------------------------------------------------------------

#[tokio::test]
async fn blocking_pops_wait_without_holding_up_other_commands() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");

    assert_eq!(redis.blpop("jobs", 0.1).await.unwrap(), None);

    let waiting = {
        let redis = redis.clone();
        tokio::spawn(async move { redis.blpop(["jobs", "other"], 5.0).await.unwrap() })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The shared connection keeps working while the pop waits...
    let started = Instant::now();
    assert!(redis.set("name", "Taylor").await.unwrap());
    assert!(started.elapsed() < Duration::from_secs(1));

    redis.rpush("jobs", "first").await.unwrap();
    assert_eq!(
        waiting.await.unwrap(),
        Some(("jobs".to_string(), "first".to_string()))
    );

    redis.rpush("jobs", ["a", "b"]).await.unwrap();
    assert_eq!(
        redis.brpop("jobs", 1.0).await.unwrap(),
        Some(("jobs".to_string(), "b".to_string()))
    );
    assert_eq!(
        redis.command("blpop", ("jobs", 1)).await.unwrap(),
        json!(["app:jobs", "a"])
    );
}

// ----------------------------------------------------------------------
// Pub / Sub
// ----------------------------------------------------------------------

#[tokio::test]
async fn messages_can_be_published_and_subscribed_to() {
    let server = RedisServer::shared();
    let redis = connection(&server, "pubsub-1:");
    let received = Arc::new(Mutex::new(Vec::new()));

    let log = received.clone();
    let subscription = redis
        .subscribe(
            ["test-channel", "other-channel"],
            move |message, channel| {
                log.lock().unwrap().push(format!("{channel}: {message}"));
            },
        )
        .await
        .unwrap();
    assert_eq!(subscription.channels(), ["test-channel", "other-channel"]);

    assert_eq!(
        redis
            .publish("test-channel", r#"{"name":"Adam Wathan"}"#)
            .await
            .unwrap(),
        1
    );
    assert_eq!(redis.publish("other-channel", "hi").await.unwrap(), 1);
    assert_eq!(redis.publish("ignored", "nobody").await.unwrap(), 0);

    // The prefix isolates channels, like keys.
    assert_eq!(raw(&redis).publish("test-channel", "no").await.unwrap(), 0);

    wait_for(|| received.lock().unwrap().len() == 2).await;
    assert_eq!(
        *received.lock().unwrap(),
        [
            r#"test-channel: {"name":"Adam Wathan"}"#,
            "other-channel: hi"
        ]
    );

    assert!(!subscription.is_finished());
    subscription.unsubscribe().await.unwrap();
    wait_for_subscribers(&redis, "test-channel", 0).await;
    assert_eq!(redis.publish("test-channel", "gone").await.unwrap(), 0);
}

#[tokio::test]
async fn wildcard_subscriptions_receive_the_channel() {
    let server = RedisServer::shared();
    let redis = connection(&server, "pubsub-2:");
    let received = Arc::new(Mutex::new(Vec::new()));

    let log = received.clone();
    let subscription = redis
        .psubscribe("users.*", move |message, channel| {
            log.lock().unwrap().push((channel, message));
        })
        .await
        .unwrap();

    redis.publish("users.1", "Taylor").await.unwrap();
    redis.publish("posts.1", "ignored").await.unwrap();
    redis.publish("users.2", "Abigail").await.unwrap();

    wait_for(|| received.lock().unwrap().len() == 2).await;
    assert_eq!(
        *received.lock().unwrap(),
        [
            ("users.1".to_string(), "Taylor".to_string()),
            ("users.2".to_string(), "Abigail".to_string())
        ]
    );

    subscription.stop();
    subscription.wait().await.unwrap();
}

async fn wait_for(condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < Duration::from_secs(5), "timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_subscribers(redis: &Connection, channel: &str, count: i64) {
    let channel = format!("{}{channel}", redis.prefix());
    let started = Instant::now();
    loop {
        let reply = raw(redis)
            .command("pubsub", ("numsub", channel.as_str()))
            .await
            .unwrap();
        if reply[1] == json!(count) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ----------------------------------------------------------------------
// The facade, the manager & configuration
// ----------------------------------------------------------------------

#[tokio::test]
async fn the_facade_uses_the_configured_connections() {
    let server = RedisServer::shared();
    let (container, _guard) = app(&server, json!({"cluster": "redis"}));

    assert!(Redis::set("name", "Taylor").await.unwrap());
    assert_eq!(Redis::get("name").await.unwrap().as_deref(), Some("Taylor"));
    assert_eq!(
        Redis::command("get", "name").await.unwrap(),
        json!("Taylor")
    );

    // The prefix defaults to the application's name...
    let default = Redis::connection(None).unwrap();
    assert_eq!(default.prefix(), "laravel-database-");
    assert!(default.same_as(&container.make::<Connection>()));
    assert_eq!(
        raw(&default)
            .get("laravel-database-name")
            .await
            .unwrap()
            .as_deref(),
        Some("Taylor")
    );

    // ...and each connection has its own database.
    let cache = Redis::connection("cache").unwrap();
    assert_ne!(cache.config().database, default.config().database);
    assert_eq!(cache.get("name").await.unwrap(), None);
    assert!(
        Redis::manager()
            .unwrap()
            .connections()
            .contains(&"cache".to_string())
    );
    assert!(Redis::connection("missing").is_err());

    // Every typed helper is on the facade too.
    assert!(Redis::set_ex("token", "abc", 60).await.unwrap());
    assert!(Redis::setnx("lock", 1).await.unwrap());
    assert_eq!(Redis::incr("visits").await.unwrap(), 1);
    assert_eq!(Redis::incrby("visits", 2).await.unwrap(), 3);
    assert_eq!(Redis::decr("visits").await.unwrap(), 2);
    assert_eq!(Redis::decrby("visits", 2).await.unwrap(), 0);
    assert!(Redis::expire("visits", 60).await.unwrap());
    assert!(Redis::ttl("visits").await.unwrap() > 0);
    assert!(Redis::mset([("a", "1")]).await.unwrap());
    assert_eq!(Redis::mget(["a"]).await.unwrap(), [Some("1".to_string())]);
    assert_eq!(Redis::exists(["a", "name"]).await.unwrap(), 2);
    assert_eq!(Redis::hset("user", "name", "Taylor").await.unwrap(), 1);
    assert_eq!(
        Redis::hget("user", "name").await.unwrap().as_deref(),
        Some("Taylor")
    );
    assert_eq!(Redis::hgetall("user").await.unwrap().len(), 1);
    assert_eq!(Redis::hdel("user", "name").await.unwrap(), 1);
    assert_eq!(Redis::rpush("list", ["a", "b"]).await.unwrap(), 2);
    assert_eq!(Redis::lpush("list", "z").await.unwrap(), 3);
    assert_eq!(Redis::lrange("list", 0, -1).await.unwrap(), ["z", "a", "b"]);
    assert_eq!(Redis::llen("list").await.unwrap(), 3);
    assert_eq!(Redis::lpop("list").await.unwrap().as_deref(), Some("z"));
    assert_eq!(Redis::rpop("list").await.unwrap().as_deref(), Some("b"));
    assert_eq!(Redis::sadd("set", ["a", "b"]).await.unwrap(), 2);
    assert_eq!(Redis::srem("set", "a").await.unwrap(), 1);
    assert_eq!(Redis::smembers("set").await.unwrap(), ["b"]);
    assert!(Redis::sismember("set", "b").await.unwrap());
    assert_eq!(Redis::zadd("board", 1.0, "a").await.unwrap(), 1);
    assert_eq!(Redis::zrange("board", 0, -1).await.unwrap(), ["a"]);
    assert_eq!(Redis::zrangebyscore("board", 0, 2).await.unwrap(), ["a"]);
    assert_eq!(Redis::zcard("board").await.unwrap(), 1);
    assert_eq!(Redis::zrem("board", "a").await.unwrap(), 1);
    assert_eq!(Redis::eval("return 1", (), ()).await.unwrap(), json!(1));
    let count: i64 = Redis::query("dbsize", ()).await.unwrap();
    assert!(count > 0);
    assert_eq!(Redis::execute_raw(("PING",)).await.unwrap(), json!("PONG"));
    assert_eq!(
        Redis::pipeline(|pipe| {
            pipe.get("name");
        })
        .await
        .unwrap(),
        [json!("Taylor")]
    );
    assert_eq!(
        Redis::transaction(|tx| {
            tx.del(["a", "token"]);
        })
        .await
        .unwrap(),
        [json!(2)]
    );
    let keys = Redis::keys("*").await.unwrap();
    assert!(keys.contains(&"name".to_string()));
    assert_eq!(Redis::del("name").await.unwrap(), 1);
    assert_eq!(Redis::publish("nobody-listens", "hi").await.unwrap(), 0);
    assert!(Redis::flushdb().await.unwrap());
}

#[tokio::test]
async fn the_facade_subscribes_on_the_default_connection() {
    let server = RedisServer::shared();
    let (_container, _guard) = app(&server, json!({"prefix": "facade-pubsub:"}));
    let received = Arc::new(Mutex::new(Vec::new()));

    let log = received.clone();
    let subscription =
        Redis::subscribe("news", move |message, _| log.lock().unwrap().push(message))
            .await
            .unwrap();
    let log = received.clone();
    let patterns = Redis::psubscribe(["news*"], move |message, _| {
        log.lock().unwrap().push(message)
    })
    .await
    .unwrap();

    Redis::publish("news", "hello").await.unwrap();
    wait_for(|| received.lock().unwrap().len() == 2).await;

    subscription.unsubscribe().await.unwrap();
    patterns.unsubscribe().await.unwrap();
}

#[tokio::test]
async fn connections_can_be_configured_with_a_url() {
    let server = RedisServer::shared();
    let database = RedisServer::next_database();
    let manager = RedisManager::new(
        "phpredis",
        json!({
            "options": {"prefix": "url:"},
            "default": {"url": server.url(database), "host": "ignored", "port": 1},
        }),
    );
    let redis = manager.connection(None).unwrap();
    assert_eq!(redis.config().database, i64::from(database));
    redis.set("name", "Taylor").await.unwrap();
    assert_eq!(redis.get("name").await.unwrap().as_deref(), Some("Taylor"));

    // Purging a connection reconnects on the next use.
    manager.purge(None);
    let fresh = manager.connection(None).unwrap();
    assert!(!fresh.same_as(&redis));
    assert_eq!(fresh.get("name").await.unwrap().as_deref(), Some("Taylor"));
}

#[tokio::test]
async fn connection_failures_are_reported() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let redis = Connection::new(
        "default",
        ConnectionConfig::parse(
            &json!({"host": "127.0.0.1", "port": port, "max_retries": 0, "timeout": 1}),
            &json!({}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(redis.get("name").await.is_err());
    assert!(redis.client().await.is_err());
}

#[tokio::test]
async fn lost_connections_are_reestablished() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");
    redis.set("name", "Taylor").await.unwrap();

    // Kill our connection from another one...
    let id = redis.command("client", "id").await.unwrap();
    raw(&redis)
        .command("client", ("kill", "id", id.to_string()))
        .await
        .unwrap();

    // ...and safe reads are retried on a fresh connection.
    assert_eq!(redis.get("name").await.unwrap().as_deref(), Some("Taylor"));
    let new_id = redis.command("client", "id").await.unwrap();
    assert_ne!(new_id, id);

    // Disconnecting explicitly works too.
    redis.disconnect();
    assert_eq!(redis.get("name").await.unwrap().as_deref(), Some("Taylor"));
    redis.client().await.unwrap();
}

#[tokio::test]
async fn passwords_are_sent_to_the_server() {
    let server = RedisServer::shared();
    let redis = connection(&server, "");
    let admin = raw(&redis);
    admin
        .command(
            "acl",
            ("setuser", "laravel-test", "on", ">secret", "~*", "+@all"),
        )
        .await
        .unwrap();

    let config = ConnectionConfig {
        username: Some("laravel-test".into()),
        password: Some("secret".into()),
        ..redis.config().clone()
    };
    let user = Connection::new("user", config.clone()).unwrap();
    assert_eq!(
        user.command("acl", "whoami").await.unwrap(),
        json!("laravel-test")
    );

    let wrong = Connection::new(
        "wrong",
        ConnectionConfig {
            password: Some("wrong".into()),
            max_retries: 0,
            ..config
        },
    )
    .unwrap();
    assert!(wrong.get("name").await.is_err());
}

// ----------------------------------------------------------------------
// Limiters
// ----------------------------------------------------------------------

#[tokio::test]
async fn throttles_allow_a_number_of_executions_per_window() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");
    let mut ran = Vec::new();

    for attempt in 1..=4 {
        let result = redis
            .throttle("key")
            .block(0)
            .allow(3)
            .every(60)
            .then(
                || async { Ok(format!("ran {attempt}")) },
                || async { Ok(format!("throttled {attempt}")) },
            )
            .await
            .unwrap();
        ran.push(result);
    }
    assert_eq!(ran, ["ran 1", "ran 2", "ran 3", "throttled 4"]);

    let error = redis
        .throttle("key")
        .block(0)
        .allow(3)
        .every(60)
        .then_or_fail(|| async { Ok(()) })
        .await
        .unwrap_err();
    assert!(error.is::<LimiterTimeoutException>());
    assert_eq!(
        error.to_string(),
        "The Redis limiter could not be acquired before the timeout."
    );

    // The limiter's state is visible...
    let mut limiter = DurationLimiter::new(redis.clone(), "key", 3, 60);
    assert!(limiter.too_many_attempts().await.unwrap());
    assert!(limiter.remaining() <= 0);
    assert!(limiter.decays_at() > 0);
    // ...and it can be cleared.
    limiter.clear().await.unwrap();
    assert!(!limiter.too_many_attempts().await.unwrap());
    assert_eq!(limiter.remaining(), 3);
    assert!(limiter.acquire().await.unwrap());
    assert_eq!(limiter.remaining(), 2);
    assert_eq!(
        raw(&redis)
            .hget("app:key", "count")
            .await
            .unwrap()
            .as_deref(),
        Some("1")
    );

    // Callback errors are passed along.
    let error = redis
        .throttle("other")
        .then(
            || async { Err::<(), _>(illuminate_support::error::error!("boom")) },
            || async { Ok(()) },
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "boom");
}

#[tokio::test]
async fn throttles_block_until_the_window_resets() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");
    let throttle = || redis.throttle("key").allow(1).every(1).sleep(50);

    throttle().then_or_fail(|| async { Ok(()) }).await.unwrap();
    assert!(
        throttle()
            .block(0)
            .then_or_fail(|| async { Ok(()) })
            .await
            .is_err()
    );

    // Blocking waits for the next window...
    let started = Instant::now();
    throttle()
        .block(5)
        .then_or_fail(|| async { Ok(()) })
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn funnels_limit_concurrent_executions() {
    let server = RedisServer::shared();
    let redis = connection(&server, "app:");
    let running = Arc::new(Mutex::new((0, 0)));

    let mut handles = Vec::new();
    for _ in 0..6 {
        let redis = redis.clone();
        let running = running.clone();
        handles.push(tokio::spawn(async move {
            redis
                .funnel("reports")
                .limit(2)
                .block(10)
                .sleep(10)
                .then_or_fail(|| async {
                    {
                        let mut running = running.lock().unwrap();
                        running.0 += 1;
                        running.1 = running.1.max(running.0);
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    running.lock().unwrap().0 -= 1;
                    Ok(())
                })
                .await
        }));
    }
    for handle in handles {
        handle.await.unwrap().unwrap();
    }
    assert_eq!(running.lock().unwrap().1, 2, "at most two ran at once");

    // Every slot was released...
    assert_eq!(redis.exists(["reports1", "reports2"]).await.unwrap(), 0);

    // ...unless it is still held.
    let limiter = ConcurrencyLimiter::new(redis.clone(), "busy", 1, 60);
    let slot = limiter.acquire("me").await.unwrap().unwrap();
    assert_eq!(slot, "busy1");
    assert_eq!(limiter.acquire("you").await.unwrap(), None);
    let outcome = redis
        .funnel("busy")
        .block(0)
        .then(|| async { Ok("ran") }, || async { Ok("busy") })
        .await
        .unwrap();
    assert_eq!(outcome, "busy");

    // Only the holder can release a slot.
    limiter.release(&slot, "you").await.unwrap();
    assert_eq!(limiter.acquire("you").await.unwrap(), None);
    limiter.release(&slot, "me").await.unwrap();
    assert_eq!(
        redis
            .funnel("busy")
            .then(|| async { Ok("ran") }, || async { Ok("busy") })
            .await
            .unwrap(),
        "ran"
    );
    let ttl = raw(&redis).ttl("app:busy1").await.unwrap();
    assert_eq!(ttl, -2, "released after running");
}

#[tokio::test]
async fn the_facade_limits_on_the_default_connection() {
    let server = RedisServer::shared();
    let (_container, _guard) = app(&server, json!({"prefix": "limits:"}));

    let first = Redis::throttle("api")
        .allow(1)
        .every(60)
        .block(0)
        .then(|| async { Ok(true) }, || async { Ok(false) })
        .await
        .unwrap();
    let second = Redis::throttle("api")
        .allow(1)
        .every(60)
        .block(0)
        .then(|| async { Ok(true) }, || async { Ok(false) })
        .await
        .unwrap();
    assert!(first && !second);

    let ran = Redis::funnel("import")
        .limit(1)
        .then(|| async { Ok(true) }, || async { Ok(false) })
        .await
        .unwrap();
    assert!(ran);
    assert_eq!(
        Redis::connection(None)
            .unwrap()
            .exists("api")
            .await
            .unwrap(),
        1
    );
}
