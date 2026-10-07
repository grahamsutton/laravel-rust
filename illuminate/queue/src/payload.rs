//! Job payloads: the JSON documents queue drivers store.
//!
//! A payload looks just like Laravel's:
//!
//! ```json
//! {
//!     "uuid": "8c4f5a1e-...",
//!     "displayName": "ProcessPodcast",
//!     "job": "Illuminate\\Queue\\CallQueuedHandler@call",
//!     "maxTries": 3,
//!     "maxExceptions": null,
//!     "failOnTimeout": false,
//!     "backoff": "1,5,10",
//!     "timeout": null,
//!     "retryUntil": null,
//!     "data": {"commandName": "app::jobs::ProcessPodcast", "command": {"podcast_id": 1}, "batchId": null},
//!     "createdAt": 1700000000,
//!     "delay": null,
//!     "attempts": 0
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use illuminate_container::try_app;
use illuminate_support::{Carbon, Map, Result, Str, Value, json};

use crate::delay::ceil_seconds;
use crate::envelope::Envelope;
use crate::exceptions::InvalidPayloadException;
use crate::manager::QueueManager;

/// The handler every object payload points at (kept for compatibility with
/// Laravel tooling).
pub const CALL_QUEUED_HANDLER: &str = "Illuminate\\Queue\\CallQueuedHandler@call";

/// A callback that adds keys to every payload: `(connection, queue, payload)`.
pub type PayloadHook = Arc<dyn Fn(&str, &str, &Value) -> Map<String, Value> + Send + Sync>;

/// Create the payload for the given job.
pub fn create_payload(
    job: &Envelope,
    connection: &str,
    queue: &str,
    delay: Option<Duration>,
) -> Result<String> {
    let payload = create_payload_value(job, connection, queue, delay)?;
    encode_payload(job, queue, &payload)
}

/// Encode a job's payload as JSON.
pub(crate) fn encode_payload(job: &Envelope, queue: &str, payload: &Value) -> Result<String> {
    serde_json::to_string(payload).map_err(|error| {
        InvalidPayloadException::new(format!(
            "Unable to JSON encode payload for job [{}] on queue [{queue}]: {error}",
            job.display_name()
        ))
        .into()
    })
}

/// Create the payload for the given job, as a JSON value.
pub fn create_payload_value(
    job: &Envelope,
    connection: &str,
    queue: &str,
    delay: Option<Duration>,
) -> Result<Value> {
    let command = job.job();
    let backoff = command.backoff();
    let backoff = (!backoff.is_empty()).then(|| {
        backoff
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    });

    let mut payload = json!({
        "uuid": Str::uuid().to_string(),
        "displayName": command.display_name(),
        "job": CALL_QUEUED_HANDLER,
        "maxTries": command.tries(),
        "maxExceptions": command.max_exceptions(),
        "failOnTimeout": command.fail_on_timeout(),
        "backoff": backoff,
        "timeout": command.timeout(),
        "retryUntil": command.retry_until().map(|until| until.timestamp()),
        "data": job.to_serialized()?,
        "createdAt": Carbon::now().timestamp(),
        "delay": delay.map(ceil_seconds),
        "attempts": 0,
    });

    if let Some(manager) = try_app::<QueueManager>() {
        for hook in manager.payload_hooks() {
            let extra = hook(connection, queue, &payload);
            if let Value::Object(map) = &mut payload {
                map.extend(extra);
            }
        }
    }

    Ok(payload)
}

/// Reset the attempts of a payload and refresh its `retryUntil`, the way
/// `queue:retry` prepares a failed job before pushing it again.
pub fn prepare_for_retry(payload: &str) -> Result<String> {
    let mut payload: Value = serde_json::from_str(payload)
        .map_err(|error| InvalidPayloadException::new(format!("Invalid job payload: {error}")))?;

    if payload.get("attempts").is_some() {
        payload["attempts"] = json!(0);
    }

    if let Some(data) = payload.get("data").cloned()
        && let Ok(data) = serde_json::from_value(data)
        && let Ok(envelope) = Envelope::from_serialized(data)
        && let Some(until) = envelope.job().retry_until()
    {
        payload["retryUntil"] = json!(until.timestamp());
    }

    Ok(payload.to_string())
}
