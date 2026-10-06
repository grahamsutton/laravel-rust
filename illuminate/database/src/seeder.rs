//! Database seeders.
//!
//! ```
//! use illuminate_database::seeder::{Seeder, call};
//! use illuminate_support::Result;
//!
//! #[derive(Default)]
//! struct UserSeeder;
//!
//! #[async_trait::async_trait]
//! impl Seeder for UserSeeder {
//!     async fn run(&self) -> Result<()> {
//!         // DB::table("users").insert(json!({...})).await?;
//!         Ok(())
//!     }
//! }
//!
//! #[derive(Default)]
//! struct DatabaseSeeder;
//!
//! #[async_trait::async_trait]
//! impl Seeder for DatabaseSeeder {
//!     async fn run(&self) -> Result<()> {
//!         call::<UserSeeder>().await
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use async_trait::async_trait;
use illuminate_support::{Result, Str};

/// A database seeder.
#[async_trait]
pub trait Seeder: Send + Sync {
    /// Seed the application's database.
    async fn run(&self) -> Result<()>;

    /// The seeder's name, as printed by `db:seed`.
    fn name(&self) -> String {
        Str::class_basename(std::any::type_name::<Self>())
    }
}

/// Progress reported while seeding.
#[derive(Clone, Debug, PartialEq)]
pub enum SeederEvent {
    /// A seeder started running.
    Running { name: String },
    /// A seeder finished.
    Done { name: String, duration_ms: f64 },
}

/// A seeding output callback.
pub type SeederOutput = Arc<dyn Fn(&SeederEvent) + Send + Sync>;

struct SeedingContext {
    output: Option<SeederOutput>,
    called: Mutex<Vec<String>>,
}

tokio::task_local! {
    static CONTEXT: Arc<SeedingContext>;
}

fn context() -> Option<Arc<SeedingContext>> {
    CONTEXT.try_with(|c| c.clone()).ok()
}

/// Run a seeder (the entry point used by `db:seed`), reporting nested calls
/// to the given output callback.
pub async fn run_seeder(seeder: &dyn Seeder, output: Option<SeederOutput>) -> Result<()> {
    let context = Arc::new(SeedingContext {
        output,
        called: Mutex::new(Vec::new()),
    });
    CONTEXT.scope(context, seeder.run()).await
}

async fn invoke(seeder: &dyn Seeder, key: String, silent: bool) -> Result<()> {
    let name = seeder.name();
    let context = context();
    let output = context
        .as_ref()
        .and_then(|c| c.output.clone())
        .filter(|_| !silent);
    if let Some(output) = &output {
        output(&SeederEvent::Running { name: name.clone() });
    }
    let start = Instant::now();
    seeder.run().await?;
    if let Some(output) = &output {
        output(&SeederEvent::Done {
            name,
            duration_ms: start.elapsed().as_secs_f64() * 1000.0,
        });
    }
    if let Some(context) = context {
        context.called.lock().unwrap().push(key);
    }
    Ok(())
}

/// Run the given seeder (`$this->call(UserSeeder::class)`).
pub async fn call<T: Seeder + Default + 'static>() -> Result<()> {
    invoke(&T::default(), std::any::type_name::<T>().to_string(), false).await
}

/// Run the given seeder without printing its progress.
pub async fn call_silent<T: Seeder + Default + 'static>() -> Result<()> {
    invoke(&T::default(), std::any::type_name::<T>().to_string(), true).await
}

/// Run the given seeder unless it has already run during this seeding.
pub async fn call_once<T: Seeder + Default + 'static>() -> Result<()> {
    let key = std::any::type_name::<T>().to_string();
    if context().is_some_and(|c| c.called.lock().unwrap().contains(&key)) {
        return Ok(());
    }
    invoke(&T::default(), key, false).await
}

/// Run several seeders, in order.
pub async fn call_many(seeders: Vec<Box<dyn Seeder>>) -> Result<()> {
    for seeder in seeders {
        let key = seeder.name();
        invoke(seeder.as_ref(), key, false).await?;
    }
    Ok(())
}

type SeederFactory = Arc<dyn Fn() -> Box<dyn Seeder> + Send + Sync>;

/// The application's seeders, by name, so `db:seed --class=UserSeeder` can
/// find them.
#[derive(Default)]
pub struct SeederRegistry {
    seeders: RwLock<HashMap<String, SeederFactory>>,
}

impl SeederRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a seeder under its class name (and its full type name).
    pub fn register<T: Seeder + Default + 'static>(&self) {
        let factory: SeederFactory = Arc::new(|| Box::new(T::default()));
        let full = std::any::type_name::<T>().to_string();
        let short = Str::class_basename(&full);
        let mut seeders = self.seeders.write().unwrap();
        seeders.insert(short, factory.clone());
        seeders.insert(full, factory);
    }

    /// Resolve a seeder by name (`DatabaseSeeder`, `UserSeeder`, ...).
    pub fn resolve(&self, name: &str) -> Option<Box<dyn Seeder>> {
        let seeders = self.seeders.read().unwrap();
        let short = Str::class_basename(&name.replace('\\', "::"));
        seeders
            .get(name)
            .or_else(|| seeders.get(&short))
            .map(|factory| factory())
    }

    /// Determine if a seeder is registered under the name.
    pub fn has(&self, name: &str) -> bool {
        self.resolve(name).is_some()
    }

    /// The names of the registered seeders (short names only).
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .seeders
            .read()
            .unwrap()
            .keys()
            .filter(|k| !k.contains("::"))
            .cloned()
            .collect();
        names.sort();
        names
    }
}
