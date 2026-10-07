//! Testing: record broadcasts instead of sending them.

use std::sync::Mutex;

use crate::contracts::{ShouldBroadcast, class_name};
use crate::event::BroadcastEvent;

/// Records broadcast events instead of queueing and sending them.
///
/// Swap it in with [`Broadcast::fake`](crate::Broadcast::fake). Broadcasts
/// are recorded the moment the event is dispatched, exactly as they would
/// have been queued: name, channels, payload, excluded socket and
/// connections.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_broadcasting::{Broadcast, Channel, ShouldBroadcast, broadcast};
/// use illuminate_container::Container;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct OrderShipped { order_id: u64 }
///
/// impl ShouldBroadcast for OrderShipped {
///     fn broadcast_on(&self) -> Vec<Channel> {
///         vec![Channel::private(format!("orders.{}", self.order_id))]
///     }
/// }
///
/// #[derive(Serialize)]
/// struct OrderCancelled;
///
/// impl ShouldBroadcast for OrderCancelled {
///     fn broadcast_on(&self) -> Vec<Channel> {
///         vec![]
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// let fake = Broadcast::fake();
///
/// broadcast(OrderShipped { order_id: 1 }).await?;
///
/// fake.assert_broadcast::<OrderShipped>();
/// fake.assert_broadcast_on::<OrderShipped>("private-orders.1");
/// fake.assert_broadcast_with::<OrderShipped>(|broadcast| broadcast.payload["order_id"] == 1);
/// fake.assert_broadcast_times::<OrderShipped>(1);
/// fake.assert_not_broadcast::<OrderCancelled>();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Debug, Default)]
pub struct BroadcastFake {
    broadcasts: Mutex<Vec<BroadcastEvent>>,
}

impl BroadcastFake {
    /// Create an empty fake.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a broadcast.
    pub fn record(&self, broadcast: BroadcastEvent) {
        self.broadcasts.lock().unwrap().push(broadcast);
    }

    /// Every recorded broadcast, in order.
    pub fn broadcasts(&self) -> Vec<BroadcastEvent> {
        self.broadcasts.lock().unwrap().clone()
    }

    /// The recorded broadcasts of the event `E`.
    pub fn broadcasted<E: ShouldBroadcast>(&self) -> Vec<BroadcastEvent> {
        let event = class_name::<E>();
        self.broadcasts()
            .into_iter()
            .filter(|broadcast| broadcast.event == event)
            .collect()
    }

    /// The recorded broadcasts with the given broadcast name.
    pub fn broadcasted_as(&self, name: &str) -> Vec<BroadcastEvent> {
        self.broadcasts()
            .into_iter()
            .filter(|broadcast| broadcast.name == name)
            .collect()
    }

    /// Determine if the event `E` was broadcast.
    pub fn has_broadcast<E: ShouldBroadcast>(&self) -> bool {
        !self.broadcasted::<E>().is_empty()
    }

    /// Forget every recorded broadcast.
    pub fn flush(&self) {
        self.broadcasts.lock().unwrap().clear();
    }

    /// Assert that the event `E` was broadcast.
    #[track_caller]
    pub fn assert_broadcast<E: ShouldBroadcast>(&self) {
        assert!(
            self.has_broadcast::<E>(),
            "The expected [{}] event was not broadcast.",
            class_name::<E>()
        );
    }

    /// Assert that a broadcast of the event `E` passing the truth test was
    /// recorded.
    #[track_caller]
    pub fn assert_broadcast_with<E: ShouldBroadcast>(
        &self,
        callback: impl Fn(&BroadcastEvent) -> bool,
    ) {
        assert!(
            self.broadcasted::<E>().iter().any(callback),
            "The expected [{}] event was not broadcast.",
            class_name::<E>()
        );
    }

    /// Assert that the event `E` was broadcast on the given channel.
    #[track_caller]
    pub fn assert_broadcast_on<E: ShouldBroadcast>(&self, channel: impl AsRef<str>) {
        let channel = channel.as_ref();
        assert!(
            self.broadcasted::<E>()
                .iter()
                .any(|broadcast| broadcast.broadcasts_on(channel)),
            "The expected [{}] event was not broadcast on the [{channel}] channel.",
            class_name::<E>()
        );
    }

    /// Assert that the event `E` was broadcast exactly `times` times.
    #[track_caller]
    pub fn assert_broadcast_times<E: ShouldBroadcast>(&self, times: usize) {
        let count = self.broadcasted::<E>().len();
        assert_eq!(
            count,
            times,
            "The expected [{}] event was broadcast {count} times instead of {times} times.",
            class_name::<E>()
        );
    }

    /// Assert that an event was broadcast with the given broadcast name.
    #[track_caller]
    pub fn assert_broadcast_as(&self, name: &str) {
        assert!(
            !self.broadcasted_as(name).is_empty(),
            "No event was broadcast as [{name}]."
        );
    }

    /// Assert that an event was broadcast with the given name and passing
    /// the truth test.
    #[track_caller]
    pub fn assert_broadcast_as_with(&self, name: &str, callback: impl Fn(&BroadcastEvent) -> bool) {
        assert!(
            self.broadcasted_as(name).iter().any(callback),
            "No matching event was broadcast as [{name}]."
        );
    }

    /// Assert that the event `E` was not broadcast.
    #[track_caller]
    pub fn assert_not_broadcast<E: ShouldBroadcast>(&self) {
        let count = self.broadcasted::<E>().len();
        assert_eq!(
            count,
            0,
            "The unexpected [{}] event was broadcast {count} times.",
            class_name::<E>()
        );
    }

    /// Assert that nothing was broadcast.
    #[track_caller]
    pub fn assert_nothing_broadcast(&self) {
        let broadcasts = self.broadcasts();
        assert!(
            broadcasts.is_empty(),
            "{} unexpected broadcasts were recorded: [{}].",
            broadcasts.len(),
            broadcasts
                .iter()
                .map(|broadcast| broadcast.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
