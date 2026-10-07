//! Pub / Sub.

use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use illuminate_container::Container;
use illuminate_support::Result;
use illuminate_support::error::RuntimeException;

/// A running subscription, listening for messages on its own connection.
///
/// The listener runs in the background until it is [stopped](Subscription::stop)
/// (dropping the handle leaves it running). Long-running listeners — an
/// Artisan command, say — simply [`wait`](Subscription::wait) on it:
///
/// ```no_run
/// use illuminate_redis::Redis;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// Redis::subscribe(["test-channel"], |message, _channel| {
///     println!("{message}");
/// })
/// .await?
/// .wait()
/// .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Subscription {
    channels: Vec<String>,
    stop: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Subscription {
    /// The channels (or patterns) being listened to.
    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    /// Stop listening. Messages already being handled finish first.
    pub fn stop(&self) {
        self.stop.notify_one();
    }

    /// Determine if the listener has stopped.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Wait until the listener stops: it was [stopped](Subscription::stop),
    /// or the connection closed.
    pub async fn wait(self) -> Result<()> {
        self.task.await.map_err(|error| {
            RuntimeException::new(format!("The Redis subscription failed: {error}")).into()
        })
    }

    /// Stop listening and wait for the listener to finish.
    pub async fn unsubscribe(self) -> Result<()> {
        self.stop();
        self.wait().await
    }
}

/// Subscribe to the channels (or patterns) on a dedicated connection and
/// hand every message to the callback as `(message, channel)`.
///
/// Channels are prefixed with the connection's prefix (like phpredis does),
/// and the prefix is removed again from the channel handed to the callback.
pub(crate) async fn listen<F>(
    client: &redis::Client,
    prefix: &str,
    channels: Vec<String>,
    patterns: bool,
    mut callback: F,
) -> Result<Subscription>
where
    F: FnMut(String, String) + Send + 'static,
{
    let (mut sink, mut messages) = client.get_async_pubsub().await?.split();
    for channel in &channels {
        let prefixed = format!("{prefix}{channel}");
        if patterns {
            sink.psubscribe(prefixed).await?;
        } else {
            sink.subscribe(prefixed).await?;
        }
    }

    let stop = Arc::new(Notify::new());
    let signal = stop.clone();
    let prefix = prefix.to_string();
    let task = tokio::spawn(Container::scope_current(async move {
        // The sink keeps the connection open for as long as we listen.
        let _sink = sink;
        loop {
            tokio::select! {
                _ = signal.notified() => break,
                message = messages.next() => {
                    let Some(message) = message else { break };
                    let channel = message.get_channel_name();
                    let channel = channel.strip_prefix(prefix.as_str()).unwrap_or(channel).to_string();
                    let payload = String::from_utf8_lossy(message.get_payload_bytes()).into_owned();
                    callback(payload, channel);
                }
            }
        }
    }));

    Ok(Subscription {
        channels,
        stop,
        task,
    })
}
