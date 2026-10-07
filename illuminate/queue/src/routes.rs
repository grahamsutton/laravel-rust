//! Queue routing: default connections and queues for job types, and
//! forwarding from one queue to another.

use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Destination {
    connection: Option<String>,
    queue: Option<String>,
}

/// The queue routes (Laravel's `QueueRoutes`).
///
/// ```
/// use illuminate_queue::QueueRoutes;
///
/// let routes = QueueRoutes::new();
/// routes.set("app::jobs::ProcessPodcast", Some("podcasts"), Some("redis"));
/// routes.forward("reports", Some("reports.fifo"), Some("sqs"));
///
/// assert_eq!(routes.queue_for("app::jobs::ProcessPodcast").as_deref(), Some("podcasts"));
/// assert_eq!(routes.connection_for("app::jobs::ProcessPodcast", None).as_deref(), Some("redis"));
/// assert_eq!(routes.forwarded_queue("reports", "sqs"), "reports.fifo");
/// assert_eq!(routes.forwarded_queue("reports", "redis"), "reports");
/// ```
#[derive(Debug, Default)]
pub struct QueueRoutes {
    routes: RwLock<HashMap<String, Destination>>,
    forwards: RwLock<HashMap<String, Destination>>,
}

impl QueueRoutes {
    /// Create an empty set of routes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Route the job registered under `job_name` to a queue and/or
    /// connection.
    pub fn set(&self, job_name: &str, queue: Option<&str>, connection: Option<&str>) {
        self.routes.write().unwrap().insert(
            job_name.to_string(),
            Destination {
                connection: connection.map(String::from),
                queue: queue.map(String::from),
            },
        );
    }

    /// Forward jobs pushed onto `queue` to another queue and/or connection.
    pub fn forward(&self, queue: &str, to: Option<&str>, connection: Option<&str>) {
        self.forwards.write().unwrap().insert(
            queue.to_string(),
            Destination {
                connection: connection.map(String::from),
                queue: to.map(String::from),
            },
        );
    }

    /// The connection a job should be sent to, when the job doesn't say.
    pub fn connection_for(&self, job_name: &str, job_queue: Option<&str>) -> Option<String> {
        if let Some(route) = self.routes.read().unwrap().get(job_name) {
            return route.connection.clone();
        }
        let queue = job_queue?;
        self.forwards
            .read()
            .unwrap()
            .get(queue)
            .and_then(|forward| forward.connection.clone())
    }

    /// The queue a job should be sent to, when the job doesn't say.
    pub fn queue_for(&self, job_name: &str) -> Option<String> {
        self.routes
            .read()
            .unwrap()
            .get(job_name)
            .and_then(|route| route.queue.clone())
    }

    /// The queue a job pushed onto `queue` (on `connection`) really goes to.
    pub fn forwarded_queue(&self, queue: &str, connection: &str) -> String {
        match self.forwards.read().unwrap().get(queue) {
            Some(forward)
                if forward
                    .connection
                    .as_deref()
                    .is_none_or(|forwarded| forwarded == connection) =>
            {
                forward.queue.clone().unwrap_or_else(|| queue.to_string())
            }
            _ => queue.to_string(),
        }
    }

    /// Determine if any routes or forwards are defined.
    pub fn is_empty(&self) -> bool {
        self.routes.read().unwrap().is_empty() && self.forwards.read().unwrap().is_empty()
    }
}
