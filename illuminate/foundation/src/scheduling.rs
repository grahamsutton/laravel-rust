//! The scheduler, wired to the rest of the framework: cache-backed mutexes
//! and scheduled queued jobs.

use async_trait::async_trait;
use illuminate_cache::Cache;
use illuminate_console::scheduling::{
    Event, EventMutex, ScheduleCache, ScheduleOutputMailer, SchedulingMutex,
};
use illuminate_queue::{Dispatchable, ShouldQueue};
use illuminate_support::{Carbon, Result, Value};

/// Prevents events from overlapping with a lock in the default cache store,
/// so it holds across `schedule:run` processes (Laravel's `CacheEventMutex`).
pub struct CacheEventMutex;

#[async_trait]
impl EventMutex for CacheEventMutex {
    async fn create(&self, event: &Event) -> bool {
        match Cache::default_store() {
            Ok(store) => store
                .add(&event.mutex_name(), true, event.expires_at() * 60)
                .await
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    async fn exists(&self, event: &Event) -> bool {
        match Cache::default_store() {
            Ok(store) => store.has(&event.mutex_name()).await.unwrap_or(false),
            Err(_) => false,
        }
    }

    async fn forget(&self, event: &Event) {
        if let Ok(store) = Cache::default_store() {
            let _ = store.forget(&event.mutex_name()).await;
        }
    }
}

/// Runs `on_one_server` events on a single server, with a lock per minute
/// in the default cache store (Laravel's `CacheSchedulingMutex`).
pub struct CacheSchedulingMutex;

impl CacheSchedulingMutex {
    fn key(event: &Event, time: &Carbon) -> String {
        format!("{}{}", event.mutex_name(), time.format("Hi"))
    }
}

#[async_trait]
impl SchedulingMutex for CacheSchedulingMutex {
    async fn create(&self, event: &Event, time: &Carbon) -> bool {
        match Cache::default_store() {
            Ok(store) => store.add(&Self::key(event, time), true, 3600).await.unwrap_or(false),
            Err(_) => false,
        }
    }

    async fn exists(&self, event: &Event, time: &Carbon) -> bool {
        match Cache::default_store() {
            Ok(store) => store.has(&Self::key(event, time)).await.unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// Keeps the scheduler's pause and interrupt signals in the default cache
/// store, so `schedule:pause` and `schedule:interrupt` reach every
/// `schedule:run` process.
pub struct CacheScheduleCache;

#[async_trait]
impl ScheduleCache for CacheScheduleCache {
    async fn get(&self, key: &str) -> Option<Value> {
        Cache::default_store().ok()?.get(key).await.ok().flatten()
    }

    async fn forever(&self, key: &str, value: Value) {
        if let Ok(store) = Cache::default_store() {
            let _ = store.forever(key, value).await;
        }
    }

    async fn forget(&self, key: &str) {
        if let Ok(store) = Cache::default_store() {
            let _ = store.forget(key).await;
        }
    }
}

/// Emails scheduled tasks' output with the default mailer
/// (`email_output_to`).
pub struct MailScheduleOutput;

#[async_trait]
impl ScheduleOutputMailer for MailScheduleOutput {
    async fn send(&self, addresses: &[String], subject: &str, output: &str) -> Result<()> {
        let addresses = addresses.to_vec();
        let subject = subject.to_string();
        illuminate_mail::Mail::raw(output, move |message| {
            message.to(addresses).subject(subject);
        })
        .await?;
        Ok(())
    }
}

/// Schedule queued jobs: `Schedule::job(Heartbeat)`.
///
/// ```ignore
/// use laravel::prelude::*;
///
/// Schedule::job(Heartbeat).every_five_minutes();
/// ```
pub trait ScheduleJobs {
    /// Dispatch the job onto the queue on schedule.
    fn job<J: ShouldQueue + Clone>(job: J) -> Event;
}

impl ScheduleJobs for illuminate_console::Schedule {
    fn job<J: ShouldQueue + Clone>(job: J) -> Event {
        let name = job.display_name();
        illuminate_console::Schedule::call(move || {
            let job = job.clone();
            async move { job.dispatch().await }
        })
        .name(name)
    }
}

/// Use the cache-backed mutexes and signals, and the mailer, for the
/// application's schedule.
pub(crate) fn boot() {
    let container = illuminate_container::Container::get_instance();
    container.singleton_if::<dyn EventMutex>(|_| std::sync::Arc::new(CacheEventMutex));
    container.singleton_if::<dyn SchedulingMutex>(|_| std::sync::Arc::new(CacheSchedulingMutex));
    container.singleton_if::<dyn ScheduleCache>(|_| std::sync::Arc::new(CacheScheduleCache));
    container.singleton_if::<dyn ScheduleOutputMailer>(|_| std::sync::Arc::new(MailScheduleOutput));
}
