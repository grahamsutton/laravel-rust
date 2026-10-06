//! The job registry: how a worker turns a payload back into a job.
//!
//! Every payload names its job (`data.commandName`). A worker looks that
//! name up in the registry to find the type to deserialize. Jobs register
//! themselves automatically the first time this process dispatches them;
//! a worker that only *processes* a job type registers it with
//! [`register_job!`](crate::register_job):
//!
//! ```
//! use illuminate_queue::{JobRegistry, ShouldQueue, async_trait, register_job};
//! use illuminate_support::Result;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct SendWelcomeEmail {
//!     user_id: u64,
//! }
//!
//! #[async_trait]
//! impl ShouldQueue for SendWelcomeEmail {
//!     async fn handle(&self) -> Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! register_job!(SendWelcomeEmail);
//!
//! assert!(JobRegistry::has(SendWelcomeEmail::job_name()));
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, RwLock};

use serde::de::DeserializeOwned;

use illuminate_support::{Result, Value};

use crate::exceptions::{InvalidPayloadException, UnknownJobException};
use crate::job::ShouldQueue;

/// How to rebuild one job type from its serialized data.
#[derive(Clone, Copy)]
pub struct JobRegistration {
    name: fn() -> &'static str,
    type_name: fn() -> &'static str,
    deserialize: fn(Value) -> Result<Arc<dyn ShouldQueue>>,
}

impl JobRegistration {
    /// The registration for the job type `T`.
    pub const fn of<T: ShouldQueue + DeserializeOwned>() -> Self {
        Self {
            name: T::job_name,
            type_name: std::any::type_name::<T>,
            deserialize: deserialize_job::<T>,
        }
    }

    /// The name the job is registered under.
    pub fn name(&self) -> &'static str {
        (self.name)()
    }

    /// The Rust type name of the job.
    pub fn type_name(&self) -> &'static str {
        (self.type_name)()
    }

    /// Deserialize a job of this type.
    pub fn deserialize(&self, command: Value) -> Result<Arc<dyn ShouldQueue>> {
        (self.deserialize)(command)
    }
}

impl fmt::Debug for JobRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobRegistration")
            .field("name", &self.name())
            .field("type", &self.type_name())
            .finish()
    }
}

fn deserialize_job<T: ShouldQueue + DeserializeOwned>(
    command: Value,
) -> Result<Arc<dyn ShouldQueue>> {
    let job: T = serde_json::from_value(command).map_err(|error| {
        InvalidPayloadException::new(format!(
            "Unable to deserialize job [{}]: {error}",
            T::job_name()
        ))
    })?;
    Ok(Arc::new(job))
}

inventory::collect!(JobRegistration);

static REGISTRY: LazyLock<RwLock<HashMap<&'static str, JobRegistration>>> = LazyLock::new(|| {
    let mut registry = HashMap::new();
    for registration in inventory::iter::<JobRegistration> {
        registry.insert(registration.name(), *registration);
    }
    RwLock::new(registry)
});

/// The process-wide registry of job types.
pub struct JobRegistry;

impl JobRegistry {
    /// Register the job type `T` so workers can deserialize it.
    pub fn register<T: ShouldQueue + DeserializeOwned>() {
        Self::add(JobRegistration::of::<T>());
    }

    /// Add a registration.
    pub fn add(registration: JobRegistration) {
        let name = registration.name();
        if REGISTRY.read().unwrap().contains_key(name) {
            return;
        }
        REGISTRY.write().unwrap().insert(name, registration);
    }

    /// Find the registration for the given job name.
    pub fn resolve(name: &str) -> Option<JobRegistration> {
        REGISTRY.read().unwrap().get(name).copied()
    }

    /// Determine if a job is registered under the given name.
    pub fn has(name: &str) -> bool {
        REGISTRY.read().unwrap().contains_key(name)
    }

    /// The names of every registered job.
    pub fn names() -> Vec<&'static str> {
        let mut names: Vec<_> = REGISTRY.read().unwrap().keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// Deserialize the job registered under `name`.
    ///
    /// Fails with an [`UnknownJobException`] when no job is registered
    /// under that name.
    pub fn deserialize(name: &str, command: Value) -> Result<Arc<dyn ShouldQueue>> {
        match Self::resolve(name) {
            Some(registration) => registration.deserialize(command),
            None => Err(UnknownJobException {
                name: name.to_string(),
            }
            .into()),
        }
    }
}

/// Register job types so queue workers can deserialize them.
///
/// Jobs register themselves when the current process dispatches them, so
/// you only need this in a worker binary that processes jobs it never
/// dispatches. A `#[derive(Job)]` macro can simply expand to this.
///
/// ```
/// use illuminate_queue::{ShouldQueue, async_trait, register_job};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct PruneStaleTags;
///
/// #[async_trait]
/// impl ShouldQueue for PruneStaleTags {
///     async fn handle(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// register_job!(PruneStaleTags);
/// ```
#[macro_export]
macro_rules! register_job {
    ($($job:ty),+ $(,)?) => {
        $(
            $crate::__private::inventory::submit! {
                $crate::JobRegistration::of::<$job>()
            }
        )+
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_trait;
    use illuminate_support::json;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct RegistryTestJob {
        id: u64,
    }

    #[async_trait]
    impl ShouldQueue for RegistryTestJob {
        async fn handle(&self) -> Result<()> {
            Ok(())
        }

        fn job_name() -> &'static str {
            "tests::RegistryTestJob"
        }
    }

    crate::register_job!(RegistryTestJob);

    #[derive(Serialize, Deserialize)]
    struct RuntimeRegisteredJob;

    #[async_trait]
    impl ShouldQueue for RuntimeRegisteredJob {
        async fn handle(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn inventory_registrations_are_found() {
        assert!(JobRegistry::has("tests::RegistryTestJob"));
        let job = JobRegistry::deserialize("tests::RegistryTestJob", json!({"id": 5})).unwrap();
        assert_eq!(job.downcast_ref::<RegistryTestJob>().unwrap().id, 5);
    }

    #[test]
    fn jobs_can_be_registered_at_runtime() {
        let name = RuntimeRegisteredJob::job_name();
        assert!(name.ends_with("RuntimeRegisteredJob"));
        JobRegistry::register::<RuntimeRegisteredJob>();
        assert!(JobRegistry::names().contains(&name));
        assert_eq!(
            JobRegistry::resolve(name).unwrap().type_name(),
            std::any::type_name::<RuntimeRegisteredJob>()
        );
    }

    #[test]
    fn unknown_jobs_fail_clearly() {
        let error = JobRegistry::deserialize("App\\Jobs\\Missing", json!({}))
            .err()
            .unwrap();
        assert!(error.is::<UnknownJobException>());
        assert!(error.to_string().contains("register_job!"));
    }

    #[test]
    fn bad_data_fails_to_deserialize() {
        let error = JobRegistry::deserialize("tests::RegistryTestJob", json!({"id": "x"}))
            .err()
            .unwrap();
        assert!(error.to_string().contains("Unable to deserialize job"));
    }
}
