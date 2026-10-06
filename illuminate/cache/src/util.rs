//! Small internal helpers.

use illuminate_support::Result;

/// Run a blocking operation off the async runtime (inline without one).
pub(crate) async fn blocking<T, F>(operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_err() {
        return operation();
    }
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| illuminate_support::error::error!("The cache task failed: {error}"))?
}
