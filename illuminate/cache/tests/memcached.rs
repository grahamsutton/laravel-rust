//! The `memcached` cache driver against small in-process Memcached servers
//! speaking the text protocol: the exact commands, expirations, key
//! distribution, locks, tags, and the manager's configuration.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use illuminate_cache::memcached::{Memcached, MemcachedServer};
use illuminate_cache::{
    Cache, CacheManager, CacheServiceProvider, LockProvider, MemcachedStore,
    Repository as CacheRepository, Store,
};
use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_support::{Carbon, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const NOW: i64 = 1_700_000_000;

/// Freeze "now" on this thread (the store and the fake servers share it).
struct Frozen;

impl Frozen {
    fn at(timestamp: i64) -> Self {
        Carbon::set_thread_test_now(Some(Carbon::from_timestamp(timestamp)));
        Self
    }

    fn travel(&self, seconds: i64) {
        Carbon::set_thread_test_now(Some(Carbon::now().add_seconds(seconds)));
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        Carbon::set_thread_test_now(None);
    }
}

#[derive(Clone)]
struct Item {
    value: Vec<u8>,
    flags: u32,
    expires_at: Option<i64>,
    cas: u64,
}

#[derive(Default)]
struct State {
    items: HashMap<String, Item>,
    commands: Vec<String>,
    next_cas: u64,
    connections: usize,
}

/// A Memcached server holding its items in memory.
#[derive(Clone)]
struct FakeMemcached {
    port: u16,
    state: Arc<Mutex<State>>,
    credentials: Option<String>,
}

impl FakeMemcached {
    async fn start() -> Self {
        Self::start_with(None).await
    }

    async fn start_with(credentials: Option<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = Self {
            port: listener.local_addr().unwrap().port(),
            state: Arc::default(),
            credentials: credentials.map(String::from),
        };
        let accepting = server.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                accepting.state.lock().unwrap().connections += 1;
                let connection = accepting.clone();
                tokio::spawn(async move { connection.serve(socket).await });
            }
        });
        server
    }

    fn config(&self) -> serde_json::Value {
        json!({"host": "127.0.0.1", "port": self.port, "weight": 100})
    }

    fn commands(&self) -> Vec<String> {
        self.state.lock().unwrap().commands.clone()
    }

    fn last_command(&self) -> String {
        self.commands().pop().unwrap()
    }

    fn item(&self, key: &str) -> Option<Item> {
        self.state.lock().unwrap().items.get(key).cloned()
    }

    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.state.lock().unwrap().items.keys().cloned().collect();
        keys.sort();
        keys
    }

    fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }

    /// Memcached reads expirations past 30 days as UNIX timestamps.
    fn expires_at(exptime: i64) -> Option<i64> {
        match exptime {
            0 => None,
            exptime if exptime > 2_592_000 => Some(exptime),
            exptime => Some(Carbon::now().timestamp() + exptime),
        }
    }

    async fn serve(&self, socket: tokio::net::TcpStream) {
        let (reader, mut writer) = socket.into_split();
        let mut reader = BufReader::new(reader);
        let mut authenticated = self.credentials.is_none();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            let line = line.trim_end().to_string();
            let parts: Vec<&str> = line.split(' ').collect();
            let data = if matches!(parts[0], "set" | "add" | "replace" | "cas") {
                let length: usize = parts[4].parse().unwrap();
                let mut data = vec![0; length + 2];
                reader.read_exact(&mut data).await.unwrap();
                data.truncate(length);
                Some(data)
            } else {
                None
            };

            if !authenticated {
                let expected = self.credentials.clone().unwrap().into_bytes();
                let reply = if parts[0] == "set" && data.as_deref() == Some(&expected[..]) {
                    authenticated = true;
                    "STORED\r\n"
                } else {
                    "CLIENT_ERROR unauthenticated\r\n"
                };
                writer.write_all(reply.as_bytes()).await.unwrap();
                continue;
            }

            let reply = self.execute(&line, &parts, data);
            writer.write_all(&reply).await.unwrap();
        }
    }

    fn execute(&self, line: &str, parts: &[&str], data: Option<Vec<u8>>) -> Vec<u8> {
        let mut state = self.state.lock().unwrap();
        state.commands.push(line.to_string());
        let now = Carbon::now().timestamp();
        state
            .items
            .retain(|_, item| item.expires_at.is_none_or(|at| at > now));
        state.next_cas += 1;
        let cas = state.next_cas;

        match parts[0] {
            "get" | "gets" => {
                let mut reply = Vec::new();
                for key in &parts[1..] {
                    if let Some(item) = state.items.get(*key) {
                        let header = if parts[0] == "gets" {
                            format!(
                                "VALUE {key} {} {} {}\r\n",
                                item.flags,
                                item.value.len(),
                                item.cas
                            )
                        } else {
                            format!("VALUE {key} {} {}\r\n", item.flags, item.value.len())
                        };
                        reply.extend(header.into_bytes());
                        reply.extend(&item.value);
                        reply.extend(b"\r\n");
                    }
                }
                reply.extend(b"END\r\n");
                reply
            }
            "set" | "add" | "replace" | "cas" => {
                let key = parts[1].to_string();
                let exists = state.items.contains_key(&key);
                let stored = match parts[0] {
                    "add" => !exists,
                    "replace" => exists,
                    "cas" => match state.items.get(&key) {
                        None => return b"NOT_FOUND\r\n".to_vec(),
                        Some(item) => item.cas.to_string() == parts[5],
                    },
                    _ => true,
                };
                if !stored {
                    return if parts[0] == "cas" {
                        b"EXISTS\r\n".to_vec()
                    } else {
                        b"NOT_STORED\r\n".to_vec()
                    };
                }
                state.items.insert(
                    key,
                    Item {
                        value: data.unwrap(),
                        flags: parts[2].parse().unwrap(),
                        expires_at: Self::expires_at(parts[3].parse().unwrap()),
                        cas,
                    },
                );
                b"STORED\r\n".to_vec()
            }
            "incr" | "decr" => {
                let Some(item) = state.items.get_mut(parts[1]) else {
                    return b"NOT_FOUND\r\n".to_vec();
                };
                let Ok(current) = String::from_utf8_lossy(&item.value).parse::<u64>() else {
                    return b"CLIENT_ERROR cannot increment or decrement non-numeric value\r\n"
                        .to_vec();
                };
                let by: u64 = parts[2].parse().unwrap();
                let new = if parts[0] == "incr" {
                    current + by
                } else {
                    current.saturating_sub(by)
                };
                item.value = new.to_string().into_bytes();
                item.cas = cas;
                format!("{new}\r\n").into_bytes()
            }
            "delete" => match state.items.remove(parts[1]) {
                Some(_) => b"DELETED\r\n".to_vec(),
                None => b"NOT_FOUND\r\n".to_vec(),
            },
            "touch" => match state.items.get_mut(parts[1]) {
                Some(item) => {
                    item.expires_at = Self::expires_at(parts[2].parse().unwrap());
                    b"TOUCHED\r\n".to_vec()
                }
                None => b"NOT_FOUND\r\n".to_vec(),
            },
            "flush_all" => {
                state.items.clear();
                b"OK\r\n".to_vec()
            }
            _ => b"ERROR\r\n".to_vec(),
        }
    }
}

fn store(server: &FakeMemcached) -> MemcachedStore {
    let memcached = Memcached::new(vec![MemcachedServer::new("127.0.0.1", server.port, 100)]);
    MemcachedStore::new(memcached, "laravel-cache-")
}

#[tokio::test]
async fn items_are_stored_with_the_exact_commands() {
    let _now = Frozen::at(NOW);
    let server = FakeMemcached::start().await;
    let store = store(&server);

    assert!(store.put("name", json!("Taylor"), 60).await.unwrap());
    assert_eq!(
        server.last_command(),
        "set laravel-cache-name 0 1700000060 6"
    );
    assert!(store.put("count", json!(42), 60).await.unwrap());
    assert_eq!(
        server.last_command(),
        "set laravel-cache-count 1 1700000060 2"
    );
    assert!(store.put("user", json!({"id": 1}), 60).await.unwrap());
    assert_eq!(
        server.item("laravel-cache-user").unwrap().value,
        br#"{"id":1}"#
    );
    assert_eq!(server.item("laravel-cache-user").unwrap().flags, 6);

    assert_eq!(store.get("name").await.unwrap(), Some(json!("Taylor")));
    assert_eq!(server.last_command(), "get laravel-cache-name");
    assert_eq!(store.get("count").await.unwrap(), Some(json!(42)));
    assert_eq!(store.get("user").await.unwrap(), Some(json!({"id": 1})));
    assert_eq!(store.get("missing").await.unwrap(), None);

    assert_eq!(
        store
            .many(&["name".into(), "missing".into(), "count".into()])
            .await
            .unwrap(),
        vec![
            ("name".to_string(), Some(json!("Taylor"))),
            ("missing".to_string(), None),
            ("count".to_string(), Some(json!(42))),
        ]
    );
    assert_eq!(
        server.last_command(),
        "get laravel-cache-name laravel-cache-missing laravel-cache-count"
    );

    // Expired items are gone.
    _now.travel(60);
    assert_eq!(store.get("name").await.unwrap(), None);

    // Every command reused the one connection.
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn forever_items_add_touch_and_forget() {
    let now = Frozen::at(NOW);
    let server = FakeMemcached::start().await;
    let store = store(&server);

    assert!(store.forever("forever", json!(true)).await.unwrap());
    assert_eq!(server.last_command(), "set laravel-cache-forever 3 0 1");
    now.travel(100_000_000);
    assert_eq!(store.get("forever").await.unwrap(), Some(json!(true)));

    assert!(store.add("once", json!("first"), 10).await.unwrap());
    assert!(
        server
            .last_command()
            .starts_with("add laravel-cache-once 0 ")
    );
    assert!(!store.add("once", json!("second"), 10).await.unwrap());
    assert_eq!(store.get("once").await.unwrap(), Some(json!("first")));

    assert!(store.touch("once", 600).await.unwrap());
    let touched = Carbon::now().timestamp() + 600;
    assert_eq!(
        server.last_command(),
        format!("touch laravel-cache-once {touched}")
    );
    assert!(!store.touch("missing", 600).await.unwrap());

    assert!(store.forget("once").await.unwrap());
    assert_eq!(server.last_command(), "delete laravel-cache-once");
    assert!(!store.forget("once").await.unwrap());

    store
        .put_many(vec![("a".into(), json!(1)), ("b".into(), json!(2))], 60)
        .await
        .unwrap();
    assert!(store.flush().await.unwrap());
    assert_eq!(server.last_command(), "flush_all");
    assert!(server.keys().is_empty());
}

#[tokio::test]
async fn counters_use_incr_and_decr() {
    let _now = Frozen::at(NOW);
    let server = FakeMemcached::start().await;
    let store = store(&server);

    assert_eq!(store.increment("visits", 1).await.unwrap(), 1);
    assert_eq!(
        server.commands()[server.commands().len() - 2..],
        [
            "incr laravel-cache-visits 1",
            "add laravel-cache-visits 1 0 1"
        ]
    );
    assert_eq!(store.increment("visits", 5).await.unwrap(), 6);
    assert_eq!(server.last_command(), "incr laravel-cache-visits 5");
    assert_eq!(store.decrement("visits", 2).await.unwrap(), 4);
    assert_eq!(server.last_command(), "decr laravel-cache-visits 2");
    assert_eq!(store.increment("visits", -1).await.unwrap(), 3);
    assert_eq!(store.get("visits").await.unwrap(), Some(json!(3)));

    // Memcached never goes below zero.
    assert_eq!(store.decrement("visits", 10).await.unwrap(), 0);
    assert_eq!(store.decrement("fresh", 10).await.unwrap(), 0);

    store.put("name", json!("Taylor"), 60).await.unwrap();
    let error = store.increment("name", 1).await.unwrap_err();
    assert!(error.to_string().contains("non-numeric"), "{error}");
    // The connection is still usable after a client error.
    assert_eq!(store.get("visits").await.unwrap(), Some(json!(0)));
}

#[tokio::test]
async fn locks_are_items_holding_their_owner() {
    let now = Frozen::at(NOW);
    let server = FakeMemcached::start().await;
    let store = store(&server);

    let lock = store.lock("reports", 10, Some("taylor".into()));
    assert_eq!(lock.name(), "laravel-cache-reports");
    assert!(lock.get().await.unwrap());
    assert_eq!(server.last_command(), "add laravel-cache-reports 0 10 6");

    let other = store.lock("reports", 10, Some("abigail".into()));
    assert!(!other.get().await.unwrap());
    assert!(lock.is_owned_by_current_process().await.unwrap());
    assert!(other.is_locked().await.unwrap());
    assert!(!other.release().await.unwrap());
    assert!(!other.refresh(None).await.unwrap());

    assert!(lock.refresh(Some(30)).await.unwrap());
    let commands = server.commands();
    assert_eq!(commands[commands.len() - 2], "gets laravel-cache-reports");
    assert!(commands[commands.len() - 1].starts_with("cas laravel-cache-reports 0 30 6 "));

    assert!(lock.release().await.unwrap());
    assert!(server.item("laravel-cache-reports").is_none());

    // Long locks get an absolute expiration.
    let long = store.lock("long", 864_001, None);
    assert!(long.acquire().await.unwrap());
    assert!(
        server
            .last_command()
            .starts_with("add laravel-cache-long 0 1700864001 ")
    );

    // Expired locks may be acquired again.
    assert!(lock.get().await.unwrap());
    now.travel(11);
    assert!(other.get().await.unwrap());
    store
        .restore_lock("reports", "nobody")
        .force_release()
        .await
        .unwrap();
    assert!(!other.is_locked().await.unwrap());
}

#[tokio::test]
async fn keys_are_spread_across_servers() {
    let first = FakeMemcached::start().await;
    let second = FakeMemcached::start().await;
    let memcached = Memcached::new(vec![
        MemcachedServer::new("127.0.0.1", first.port, 100),
        MemcachedServer::new("127.0.0.1", second.port, 100),
    ]);
    let store = MemcachedStore::new(memcached.clone(), "");

    let keys: Vec<String> = (0..40).map(|i| format!("key-{i}")).collect();
    for key in &keys {
        store.forever(key, json!(key)).await.unwrap();
    }
    assert!(!first.keys().is_empty() && !second.keys().is_empty());
    assert_eq!(first.keys().len() + second.keys().len(), 40);
    for key in first.keys() {
        assert_eq!(memcached.server_index(&key), 0);
    }

    // `many` asks each server for its own keys.
    let values = store.many(&keys).await.unwrap();
    assert!(
        values
            .iter()
            .all(|(key, value)| value.as_ref() == Some(&json!(key)))
    );
    assert!(first.last_command().starts_with("get "));
    assert!(second.last_command().starts_with("get "));

    store.flush().await.unwrap();
    assert!(first.keys().is_empty() && second.keys().is_empty());
}

#[tokio::test]
async fn connections_authenticate_and_errors_are_reported() {
    let server = FakeMemcached::start_with(Some("taylor secret")).await;
    let memcached = Memcached::from_config(&json!({
        "servers": [server.config()],
        "sasl": ["taylor", "secret"],
    }))
    .unwrap();
    let store = MemcachedStore::new(memcached, "");
    assert!(store.put("a", json!(1), 60).await.unwrap());
    assert_eq!(store.get("a").await.unwrap(), Some(json!(1)));

    let wrong = Memcached::from_config(&json!({
        "servers": [server.config()],
        "sasl": ["taylor", "wrong"],
    }))
    .unwrap();
    let error = MemcachedStore::new(wrong, "").get("a").await.unwrap_err();
    assert!(error.to_string().contains("unauthenticated"), "{error}");

    let invalid = store.get("has spaces").await.unwrap_err();
    assert!(invalid.to_string().contains("not a valid Memcached key"));

    // Nobody listens on the port of a server that went away.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let offline = Memcached::new(vec![MemcachedServer::new("127.0.0.1", port, 1)]);
    let error = MemcachedStore::new(offline, "").get("a").await.unwrap_err();
    assert!(
        error.to_string().contains("Unable to connect to Memcached"),
        "{error}"
    );
}

#[tokio::test]
async fn the_manager_builds_memcached_stores_with_tags() {
    let _now = Frozen::at(NOW);
    let server = FakeMemcached::start().await;
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "app": {"name": "Laravel"},
        "cache": {
            "default": "memcached",
            "stores": {
                "memcached": {
                    "driver": "memcached",
                    "persistent_id": null,
                    "sasl": [null, null],
                    "options": {},
                    "servers": [server.config()],
                },
            },
            "prefix": "laravel-cache-",
        },
    })));
    CacheServiceProvider.register(&container);

    Cache::put("name", "Taylor", 60).await.unwrap();
    assert_eq!(
        server.last_command(),
        "set laravel-cache-name 0 1700000060 6"
    );
    assert_eq!(Cache::string("name").await.unwrap(), "Taylor");
    assert_eq!(Cache::increment("hits").await.unwrap(), 1);
    assert!(Cache::lock("reports", 10).unwrap().get().await.unwrap());

    let manager = container.make::<CacheManager>();
    let cache: CacheRepository = manager.store("memcached").unwrap();
    assert!(cache.supports_tags());
    let tagged = cache.tags(["people", "artists"]).unwrap();
    tagged.put("john", "Doe", 60).await.unwrap();
    assert_eq!(tagged.get("john").await.unwrap(), Some(json!("Doe")));
    tagged.flush().await.unwrap();
    assert_eq!(tagged.get("john").await.unwrap(), None);

    assert_eq!(
        manager
            .build(json!({"driver": "memcached", "servers": []}))
            .unwrap_err()
            .to_string(),
        "The memcached cache driver requires at least one server."
    );
}
