//! The `beanstalkd` driver against a small in-process Beanstalkd server
//! speaking the protocol: the exact commands, reservations, delays,
//! releases, burying, sizes, blocking pops, and the worker loop.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{TestApp, app_with, record, recorded};
use illuminate_queue::contracts::Queue as QueueContract;
use illuminate_queue::{
    BeanstalkdQueue, Dispatchable, InteractsWithQueue, Queue, ShouldQueue, Worker, WorkerOptions,
    async_trait,
};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, json};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const NOW: i64 = 1_700_000_000;

// ----------------------------------------------------------------------
// A fake Beanstalkd
// ----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum JobState {
    Ready,
    Delayed(i64),
    Reserved(u64),
    Buried,
}

#[derive(Clone, Debug)]
struct Job {
    tube: String,
    data: String,
    priority: u32,
    ttr: u64,
    state: JobState,
    reserves: u32,
    releases: u32,
}

#[derive(Default)]
struct State {
    jobs: BTreeMap<u64, Job>,
    tubes: Vec<String>,
    commands: Vec<(u64, String)>,
    next_id: u64,
}

/// A Beanstalkd server holding its jobs in memory.
#[derive(Clone)]
struct FakeBeanstalkd {
    port: u16,
    state: Arc<Mutex<State>>,
    connections: Arc<AtomicU64>,
}

impl FakeBeanstalkd {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = Self {
            port: listener.local_addr().unwrap().port(),
            state: Arc::default(),
            connections: Arc::default(),
        };
        let accepting = server.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let id = accepting.connections.fetch_add(1, Ordering::SeqCst) + 1;
                let connection = accepting.clone();
                tokio::spawn(async move { connection.serve(id, socket).await });
            }
        });
        server
    }

    /// The commands every connection sent, in order.
    fn commands(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .commands
            .iter()
            .map(|(_, command)| command.clone())
            .collect()
    }

    /// The commands sent since the given number of commands.
    fn commands_since(&self, count: usize) -> Vec<String> {
        self.commands()[count..].to_vec()
    }

    fn job(&self, id: u64) -> Option<Job> {
        self.state.lock().unwrap().jobs.get(&id).cloned()
    }

    fn jobs(&self) -> Vec<Job> {
        self.state.lock().unwrap().jobs.values().cloned().collect()
    }

    /// Make delayed jobs whose time has come ready.
    fn promote(state: &mut State) {
        let now = Carbon::now().timestamp();
        for job in state.jobs.values_mut() {
            if let JobState::Delayed(at) = job.state
                && at <= now
            {
                job.state = JobState::Ready;
            }
        }
    }

    fn try_reserve(&self, connection: u64, watching: &[String]) -> Option<(u64, String)> {
        let mut state = self.state.lock().unwrap();
        Self::promote(&mut state);
        let (id, job) = state
            .jobs
            .iter_mut()
            .filter(|(_, job)| job.state == JobState::Ready && watching.contains(&job.tube))
            .min_by_key(|(id, job)| (job.priority, **id))?;
        job.state = JobState::Reserved(connection);
        job.reserves += 1;
        Some((*id, job.data.clone()))
    }

    async fn serve(&self, connection: u64, socket: tokio::net::TcpStream) {
        let (reader, mut writer) = socket.into_split();
        let mut reader = BufReader::new(reader);
        let mut using = "default".to_string();
        let mut watching = vec!["default".to_string()];
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                // Jobs reserved by a closed connection go back to the tube.
                let mut state = self.state.lock().unwrap();
                for job in state.jobs.values_mut() {
                    if job.state == JobState::Reserved(connection) {
                        job.state = JobState::Ready;
                    }
                }
                return;
            }
            let line = line.trim_end().to_string();
            self.state
                .lock()
                .unwrap()
                .commands
                .push((connection, line.clone()));
            let parts: Vec<&str> = line.split(' ').collect();

            let reply = match parts[0] {
                "use" => {
                    using = parts[1].to_string();
                    format!("USING {using}\r\n")
                }
                "watch" => {
                    if !watching.contains(&parts[1].to_string()) {
                        watching.push(parts[1].to_string());
                    }
                    format!("WATCHING {}\r\n", watching.len())
                }
                "ignore" => {
                    if watching.len() == 1 {
                        "NOT_IGNORED\r\n".to_string()
                    } else {
                        watching.retain(|tube| tube != parts[1]);
                        format!("WATCHING {}\r\n", watching.len())
                    }
                }
                "put" => {
                    let length: usize = parts[4].parse().unwrap();
                    let mut data = vec![0; length + 2];
                    reader.read_exact(&mut data).await.unwrap();
                    data.truncate(length);
                    let delay: i64 = parts[2].parse().unwrap();
                    let mut state = self.state.lock().unwrap();
                    state.next_id += 1;
                    let id = state.next_id;
                    if !state.tubes.contains(&using) {
                        state.tubes.push(using.clone());
                    }
                    state.jobs.insert(
                        id,
                        Job {
                            tube: using.clone(),
                            data: String::from_utf8(data).unwrap(),
                            priority: parts[1].parse().unwrap(),
                            ttr: parts[3].parse().unwrap(),
                            state: if delay > 0 {
                                JobState::Delayed(Carbon::now().timestamp() + delay)
                            } else {
                                JobState::Ready
                            },
                            reserves: 0,
                            releases: 0,
                        },
                    );
                    format!("INSERTED {id}\r\n")
                }
                "reserve-with-timeout" => {
                    let timeout: u64 = parts[1].parse().unwrap();
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
                    loop {
                        if let Some((id, data)) = self.try_reserve(connection, &watching) {
                            break format!("RESERVED {id} {}\r\n{data}\r\n", data.len());
                        }
                        if tokio::time::Instant::now() >= deadline {
                            break "TIMED_OUT\r\n".to_string();
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
                "delete" | "release" | "bury" => {
                    let id: u64 = parts[1].parse().unwrap();
                    let mut state = self.state.lock().unwrap();
                    let owned = state.jobs.get(&id).is_some_and(|job| match job.state {
                        JobState::Reserved(by) => by == connection,
                        _ => parts[0] == "delete",
                    });
                    if !owned {
                        "NOT_FOUND\r\n".to_string()
                    } else if parts[0] == "delete" {
                        state.jobs.remove(&id);
                        "DELETED\r\n".to_string()
                    } else if parts[0] == "bury" {
                        let job = state.jobs.get_mut(&id).unwrap();
                        job.state = JobState::Buried;
                        "BURIED\r\n".to_string()
                    } else {
                        let job = state.jobs.get_mut(&id).unwrap();
                        let delay: i64 = parts[3].parse().unwrap();
                        job.priority = parts[2].parse().unwrap();
                        job.releases += 1;
                        job.state = if delay > 0 {
                            JobState::Delayed(Carbon::now().timestamp() + delay)
                        } else {
                            JobState::Ready
                        };
                        "RELEASED\r\n".to_string()
                    }
                }
                "stats-job" => {
                    let id: u64 = parts[1].parse().unwrap();
                    let state = self.state.lock().unwrap();
                    match state.jobs.get(&id) {
                        None => "NOT_FOUND\r\n".to_string(),
                        Some(job) => {
                            let yaml = format!(
                                "---\nid: {id}\ntube: {}\nstate: {:?}\npri: {}\nttr: {}\nreserves: {}\nreleases: {}\n",
                                job.tube,
                                job.state,
                                job.priority,
                                job.ttr,
                                job.reserves,
                                job.releases
                            );
                            format!("OK {}\r\n{yaml}\r\n", yaml.len())
                        }
                    }
                }
                "stats-tube" => {
                    let mut state = self.state.lock().unwrap();
                    Self::promote(&mut state);
                    if !state.tubes.contains(&parts[1].to_string()) {
                        "NOT_FOUND\r\n".to_string()
                    } else {
                        let count = |predicate: &dyn Fn(&JobState) -> bool| {
                            state
                                .jobs
                                .values()
                                .filter(|job| job.tube == parts[1] && predicate(&job.state))
                                .count()
                        };
                        let yaml = format!(
                            "---\nname: {}\ncurrent-jobs-urgent: 0\ncurrent-jobs-ready: {}\ncurrent-jobs-reserved: {}\ncurrent-jobs-delayed: {}\ncurrent-jobs-buried: {}\n",
                            parts[1],
                            count(&|s| *s == JobState::Ready),
                            count(&|s| matches!(s, JobState::Reserved(_))),
                            count(&|s| matches!(s, JobState::Delayed(_))),
                            count(&|s| *s == JobState::Buried),
                        );
                        format!("OK {}\r\n{yaml}\r\n", yaml.len())
                    }
                }
                _ => "UNKNOWN_COMMAND\r\n".to_string(),
            };
            if writer.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
        }
    }
}

// ----------------------------------------------------------------------
// Setup
// ----------------------------------------------------------------------

fn app(server: &FakeBeanstalkd, block_for: u64) -> TestApp {
    let mut config = common::config();
    config["queue"]["default"] = json!("beanstalkd");
    config["queue"]["connections"]["beanstalkd"] = json!({
        "driver": "beanstalkd",
        "host": "127.0.0.1",
        "port": server.port,
        "queue": "default",
        "retry_after": 90,
        "block_for": block_for,
        "after_commit": false,
    });
    app_with(config)
}

fn queue() -> Arc<dyn QueueContract> {
    Queue::connection("beanstalkd").unwrap()
}

/// Freeze "now" on this thread (the queue and the fake share it).
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

fn decode(payload: &str) -> Value {
    serde_json::from_str(payload).unwrap()
}

// ----------------------------------------------------------------------
// Jobs
// ----------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct SendInvoice {
    id: u64,
}

#[async_trait]
impl ShouldQueue for SendInvoice {
    async fn handle(&self) -> Result<()> {
        record(format!("invoice:{} attempt:{}", self.id, self.attempts()));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct FlakyJob;

#[async_trait]
impl ShouldQueue for FlakyJob {
    async fn handle(&self) -> Result<()> {
        record(format!("flaky attempt:{}", self.attempts()));
        if self.attempts() < 2 {
            return Err(RuntimeException::new("Try again").into());
        }
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(3)
    }
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[tokio::test]
async fn jobs_are_put_into_tubes() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 0);

    SendInvoice { id: 1 }.dispatch().await.unwrap();
    let commands = server.commands();
    assert_eq!(commands.len(), 1);
    assert!(commands[0].starts_with("put 1024 0 90 "), "{commands:?}");
    let job = server.job(1).unwrap();
    assert_eq!(job.tube, "default");
    assert_eq!(job.ttr, 90);
    let payload = decode(&job.data);
    assert_eq!(payload["displayName"], "SendInvoice");
    assert_eq!(payload["data"]["command"], json!({"id": 1}));

    let id = queue()
        .push_raw(
            r#"{"uuid":"raw","job":"x","data":{}}"#.into(),
            Some("emails"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(id.as_deref(), Some("2"));
    assert_eq!(
        server.commands_since(1),
        vec![
            "use emails".to_string(),
            format!(
                "put 1024 0 90 {}",
                r#"{"uuid":"raw","job":"x","data":{}}"#.len()
            )
        ]
    );
    assert_eq!(server.job(2).unwrap().tube, "emails");

    SendInvoice { id: 3 }
        .dispatch()
        .on_queue("emails")
        .await
        .unwrap();
    assert!(
        server
            .commands()
            .last()
            .unwrap()
            .starts_with("put 1024 0 90 "),
        "the tube is still in use"
    );
    assert_eq!(server.commands().len(), 4);
}

#[tokio::test]
async fn delayed_jobs_are_put_with_a_delay() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 0);
    let now = Frozen::at(NOW);

    SendInvoice { id: 1 }.dispatch().delay(60).await.unwrap();
    assert!(server.commands()[0].starts_with("put 1024 60 90 "));
    assert_eq!(server.job(1).unwrap().state, JobState::Delayed(NOW + 60));

    assert!(queue().pop(None).await.unwrap().is_none());
    assert_eq!(queue().delayed_size(None).await.unwrap(), 1);
    assert_eq!(queue().pending_size(None).await.unwrap(), 0);

    now.travel(60);
    let job = queue().pop(None).await.unwrap().unwrap();
    assert_eq!(decode(job.raw_body())["data"]["command"], json!({"id": 1}));
}

#[tokio::test]
async fn popped_jobs_count_their_reservations() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 0);
    let now = Frozen::at(NOW);
    SendInvoice { id: 7 }
        .dispatch()
        .on_queue("emails")
        .await
        .unwrap();
    let sent = server.commands().len();

    let job = queue().pop(Some("emails")).await.unwrap().unwrap();
    assert_eq!(
        server.commands_since(sent),
        vec![
            "watch emails",
            "ignore default",
            "reserve-with-timeout 0",
            "stats-job 1"
        ]
    );
    assert_eq!(job.job_id(), "1");
    assert_eq!(job.attempts(), 1);
    assert_eq!(job.queue(), "emails");
    assert_eq!(job.connection_name(), "beanstalkd");
    assert_eq!(queue().reserved_size(Some("emails")).await.unwrap(), 1);

    // Nobody else gets a reserved job.
    assert!(queue().pop(Some("emails")).await.unwrap().is_none());

    let sent = server.commands().len();
    job.release(10).await.unwrap();
    assert_eq!(server.commands_since(sent), vec!["release 1 1024 10"]);
    assert_eq!(server.job(1).unwrap().state, JobState::Delayed(NOW + 10));
    assert!(queue().pop(Some("emails")).await.unwrap().is_none());

    now.travel(10);
    let job = queue().pop(Some("emails")).await.unwrap().unwrap();
    assert_eq!(job.attempts(), 2);

    let sent = server.commands().len();
    job.delete().await.unwrap();
    assert_eq!(server.commands_since(sent), vec!["delete 1"]);
    assert!(server.jobs().is_empty());
    assert_eq!(queue().size(Some("emails")).await.unwrap(), 0);
}

#[tokio::test]
async fn sizes_come_from_the_tube_stats() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 0);
    let _now = Frozen::at(NOW);

    assert_eq!(
        queue().size(None).await.unwrap(),
        0,
        "unknown tubes are empty"
    );
    for id in 0..3 {
        SendInvoice { id }.dispatch().await.unwrap();
    }
    SendInvoice { id: 9 }.dispatch().delay(30).await.unwrap();
    let _reserved = queue().pop(None).await.unwrap().unwrap();

    let sent = server.commands().len();
    assert_eq!(queue().size(None).await.unwrap(), 4);
    assert_eq!(server.commands_since(sent), vec!["stats-tube default"]);
    assert_eq!(queue().pending_size(None).await.unwrap(), 2);
    assert_eq!(queue().delayed_size(None).await.unwrap(), 1);
    assert_eq!(queue().reserved_size(None).await.unwrap(), 1);
    assert_eq!(Queue::size(None).await.unwrap(), 4);
}

#[tokio::test]
async fn jobs_can_be_buried_and_deleted_by_id() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 0);
    let queue = BeanstalkdQueue::from_config(
        &json!({"host": "127.0.0.1", "port": server.port, "retry_after": 30}),
        "beanstalkd",
    );

    queue
        .push_raw(r#"{"uuid":"1","data":{}}"#.into(), None, None)
        .await
        .unwrap();
    let job = queue.pop(None).await.unwrap().unwrap();
    assert!(queue.bury(&job).await.unwrap());
    assert_eq!(server.commands().last().unwrap(), "bury 1 1024");
    assert_eq!(server.job(1).unwrap().state, JobState::Buried);
    assert_eq!(server.job(1).unwrap().ttr, 30);
    assert!(queue.pop(None).await.unwrap().is_none());

    assert!(queue.delete_message(None, 1).await.unwrap());
    assert!(!queue.delete_message(None, 1).await.unwrap());
    assert!(server.jobs().is_empty());

    assert_eq!(
        queue.clear(None).await.unwrap_err().to_string(),
        "The [beanstalkd] queue connection does not support clearing queues."
    );
}

#[tokio::test]
async fn pops_wait_for_jobs_with_block_for() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 2);

    let pusher = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let connection = queue();
    let (job, _) = tokio::join!(connection.pop(None), async {
        pusher.await.unwrap();
        queue()
            .push_raw(r#"{"uuid":"late","data":{}}"#.into(), None, None)
            .await
            .unwrap();
    });
    let job = job
        .unwrap()
        .expect("the job pushed while waiting is reserved");
    assert_eq!(job.uuid(), Some("late"));
    assert!(
        server
            .commands()
            .contains(&"reserve-with-timeout 2".to_string())
    );
}

#[tokio::test]
async fn the_worker_processes_releases_and_deletes_jobs() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 0);

    SendInvoice { id: 1 }.dispatch().await.unwrap();
    FlakyJob.dispatch().await.unwrap();

    let worker = Worker::make();
    worker
        .daemon(
            "beanstalkd",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();

    assert_eq!(
        recorded(),
        vec!["invoice:1 attempt:1", "flaky attempt:1", "flaky attempt:2"]
    );
    assert!(server.jobs().is_empty());
    assert!(
        server
            .commands()
            .iter()
            .any(|command| command == "release 2 1024 0")
    );
}

#[tokio::test]
async fn unreadable_jobs_are_deleted_and_logged_as_failed() {
    let server = FakeBeanstalkd::start().await;
    let _app = app(&server, 0);
    queue()
        .push_raw("not json".into(), None, None)
        .await
        .unwrap();

    assert!(queue().pop(None).await.is_err());
    assert!(server.jobs().is_empty());
    let failed = illuminate_queue::failed::failer().all().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].payload, "not json");
    assert_eq!(failed[0].connection, "beanstalkd");
    assert_eq!(failed[0].queue, "default");
}

#[tokio::test]
async fn connection_failures_are_reported_and_recovered_from() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let queue =
        BeanstalkdQueue::from_config(&json!({"host": "127.0.0.1", "port": port}), "beanstalkd");
    let error = queue.push_raw("{}".into(), None, None).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Unable to connect to Beanstalkd"),
        "{error}"
    );

    let error = queue
        .push_raw("{}".into(), Some("bad tube"), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("is not valid"), "{error}");
}
