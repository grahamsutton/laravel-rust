//! Event fakes: record dispatched events instead of running listeners, then
//! make assertions about them.

use std::any::{TypeId, type_name};
use std::sync::{Arc, Mutex, RwLock};

use indexmap::IndexMap;

use illuminate_support::{Str, Value};

use crate::dispatcher::Dispatcher;
use crate::listener::AnyEvent;

/// Identifies an event (or set of named events) for faking.
#[derive(Debug, Clone)]
pub(crate) enum Matcher {
    Type(TypeId),
    Name(String),
}

enum Recorded {
    Typed {
        type_id: TypeId,
        name: &'static str,
        event: AnyEvent,
    },
    Named {
        name: String,
        payload: Value,
    },
}

impl Recorded {
    fn name(&self) -> &str {
        match self {
            Recorded::Typed { name, .. } => name,
            Recorded::Named { name, .. } => name,
        }
    }
}

/// The state carried by a faked dispatcher.
pub(crate) struct EventFake {
    pub(crate) original: Arc<Dispatcher>,
    to_fake: RwLock<Vec<Matcher>>,
    to_dispatch: RwLock<Vec<Matcher>>,
    events: Mutex<Vec<Recorded>>,
}

impl EventFake {
    fn matches(matchers: &[Matcher], type_id: Option<TypeId>, name: Option<&str>) -> bool {
        matchers.iter().any(|matcher| match (matcher, type_id, name) {
            (Matcher::Type(expected), Some(actual), _) => *expected == actual,
            (Matcher::Name(pattern), _, Some(name)) => Str::is(pattern, name),
            _ => false,
        })
    }

    fn should_fake(&self, type_id: Option<TypeId>, name: Option<&str>) -> bool {
        if Self::matches(&self.to_dispatch.read().unwrap(), type_id, name) {
            return false;
        }
        let to_fake = self.to_fake.read().unwrap();
        to_fake.is_empty() || Self::matches(&to_fake, type_id, name)
    }

    pub(crate) fn should_fake_typed(&self, type_id: TypeId) -> bool {
        self.should_fake(Some(type_id), None)
    }

    pub(crate) fn should_fake_named(&self, name: &str) -> bool {
        self.should_fake(None, Some(name))
    }

    pub(crate) fn record_typed(&self, type_id: TypeId, name: &'static str, event: AnyEvent) {
        self.events.lock().unwrap().push(Recorded::Typed {
            type_id,
            name,
            event,
        });
    }

    pub(crate) fn record_named(&self, name: &str, payload: Value) {
        self.events.lock().unwrap().push(Recorded::Named {
            name: name.to_string(),
            payload,
        });
    }
}

fn times(count: usize) -> &'static str {
    if count == 1 { "time" } else { "times" }
}

impl Dispatcher {
    /// Create a fake dispatcher wrapping the given (real) dispatcher.
    ///
    /// When `to_fake` is empty every event is faked; otherwise only the
    /// listed events are, and the rest are dispatched as normal.
    pub(crate) fn fake_of(original: Arc<Dispatcher>, to_fake: Vec<Matcher>) -> Dispatcher {
        // Never wrap a fake in a fake: forward to the real dispatcher.
        let original = match &original.fake {
            Some(fake) => fake.original.clone(),
            None => original,
        };
        let mut dispatcher = Dispatcher::default();
        dispatcher.fake = Some(EventFake {
            original,
            to_fake: RwLock::new(to_fake),
            to_dispatch: RwLock::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        });
        dispatcher
    }

    /// Determine if this dispatcher is a fake.
    pub fn is_fake(&self) -> bool {
        self.fake.is_some()
    }

    /// The real dispatcher a fake stands in front of (`None` when this
    /// dispatcher isn't a fake).
    pub fn original(&self) -> Option<Arc<Dispatcher>> {
        self.fake.as_ref().map(|fake| fake.original.clone())
    }

    #[track_caller]
    fn fake_state(&self) -> &EventFake {
        self.fake
            .as_ref()
            .expect("Event assertions require a fake dispatcher. Did you forget to call Event::fake()?")
    }

    /// Dispatch the event `E` normally instead of faking it.
    ///
    /// ```
    /// use illuminate_events::Event;
    /// # use std::sync::Arc;
    /// # use illuminate_container::Container;
    /// # let container = Arc::new(Container::new());
    /// # let _guard = Container::set_local_instance(container);
    /// struct OrderCreated;
    ///
    /// Event::fake().except::<OrderCreated>();
    /// ```
    #[track_caller]
    pub fn except<E: 'static>(&self) -> &Self {
        self.fake_state()
            .to_dispatch
            .write()
            .unwrap()
            .push(Matcher::Type(TypeId::of::<E>()));
        self
    }

    /// Dispatch the matching named events normally instead of faking them.
    #[track_caller]
    pub fn except_named(&self, event: &str) -> &Self {
        self.fake_state()
            .to_dispatch
            .write()
            .unwrap()
            .push(Matcher::Name(event.to_string()));
        self
    }

    /// Add the event `E` to the subset of events being faked.
    #[track_caller]
    pub fn also_fake<E: 'static>(&self) -> &Self {
        self.fake_state()
            .to_fake
            .write()
            .unwrap()
            .push(Matcher::Type(TypeId::of::<E>()));
        self
    }

    /// Add matching named events to the subset of events being faked.
    #[track_caller]
    pub fn also_fake_named(&self, event: &str) -> &Self {
        self.fake_state()
            .to_fake
            .write()
            .unwrap()
            .push(Matcher::Name(event.to_string()));
        self
    }

    /// Get every recorded event of type `E`, in dispatch order.
    #[track_caller]
    pub fn dispatched<E: Send + Sync + 'static>(&self) -> Vec<Arc<E>> {
        let id = TypeId::of::<E>();
        self.fake_state()
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|recorded| match recorded {
                Recorded::Typed { type_id, event, .. } if *type_id == id => {
                    event.clone().downcast::<E>().ok()
                }
                _ => None,
            })
            .collect()
    }

    /// Get the payloads of every recorded named event matching the name.
    #[track_caller]
    pub fn dispatched_named(&self, event: &str) -> Vec<Value> {
        self.fake_state()
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|recorded| match recorded {
                Recorded::Named { name, payload } if Str::is(event, name) => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }

    /// Determine if the event `E` has been dispatched.
    #[track_caller]
    pub fn has_dispatched<E: Send + Sync + 'static>(&self) -> bool {
        !self.dispatched::<E>().is_empty()
    }

    /// Determine if a named event has been dispatched.
    #[track_caller]
    pub fn has_dispatched_named(&self, event: &str) -> bool {
        !self.dispatched_named(event).is_empty()
    }

    /// The names of every recorded event, in dispatch order.
    #[track_caller]
    pub fn dispatched_events(&self) -> Vec<String> {
        self.fake_state()
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|recorded| recorded.name().to_string())
            .collect()
    }

    // ------------------------------------------------------------------
    // Assertions
    // ------------------------------------------------------------------

    /// Assert that the event `E` was dispatched.
    #[track_caller]
    pub fn assert_dispatched<E: Send + Sync + 'static>(&self) {
        assert!(
            self.has_dispatched::<E>(),
            "The expected [{}] event was not dispatched.",
            type_name::<E>()
        );
    }

    /// Assert that an event `E` passing the given truth test was dispatched.
    #[track_caller]
    pub fn assert_dispatched_with<E: Send + Sync + 'static>(&self, callback: impl Fn(&E) -> bool) {
        assert!(
            self.dispatched::<E>().iter().any(|event| callback(event)),
            "The expected [{}] event was not dispatched.",
            type_name::<E>()
        );
    }

    /// Assert that the event `E` was dispatched exactly `times` times.
    #[track_caller]
    pub fn assert_dispatched_times<E: Send + Sync + 'static>(&self, expected: usize) {
        let count = self.dispatched::<E>().len();
        assert_eq!(
            count,
            expected,
            "The expected [{}] event was dispatched {count} {} instead of {expected} {}.",
            type_name::<E>(),
            times(count),
            times(expected)
        );
    }

    /// Assert that the event `E` was dispatched exactly once.
    #[track_caller]
    pub fn assert_dispatched_once<E: Send + Sync + 'static>(&self) {
        self.assert_dispatched_times::<E>(1);
    }

    /// Assert that the event `E` was not dispatched.
    #[track_caller]
    pub fn assert_not_dispatched<E: Send + Sync + 'static>(&self) {
        assert!(
            !self.has_dispatched::<E>(),
            "The unexpected [{}] event was dispatched.",
            type_name::<E>()
        );
    }

    /// Assert that no event `E` passing the given truth test was dispatched.
    #[track_caller]
    pub fn assert_not_dispatched_with<E: Send + Sync + 'static>(&self, callback: impl Fn(&E) -> bool) {
        assert!(
            !self.dispatched::<E>().iter().any(|event| callback(event)),
            "The unexpected [{}] event was dispatched.",
            type_name::<E>()
        );
    }

    /// Assert that a named event was dispatched.
    #[track_caller]
    pub fn assert_dispatched_named(&self, event: &str) {
        assert!(
            self.has_dispatched_named(event),
            "The expected [{event}] event was not dispatched."
        );
    }

    /// Assert that a named event passing the given truth test was dispatched.
    #[track_caller]
    pub fn assert_dispatched_named_with(&self, event: &str, callback: impl Fn(&Value) -> bool) {
        assert!(
            self.dispatched_named(event).iter().any(callback),
            "The expected [{event}] event was not dispatched."
        );
    }

    /// Assert that a named event was dispatched exactly `times` times.
    #[track_caller]
    pub fn assert_dispatched_named_times(&self, event: &str, expected: usize) {
        let count = self.dispatched_named(event).len();
        assert_eq!(
            count,
            expected,
            "The expected [{event}] event was dispatched {count} {} instead of {expected} {}.",
            times(count),
            times(expected)
        );
    }

    /// Assert that a named event was not dispatched.
    #[track_caller]
    pub fn assert_not_dispatched_named(&self, event: &str) {
        assert!(
            !self.has_dispatched_named(event),
            "The unexpected [{event}] event was dispatched."
        );
    }

    /// Assert that no events were dispatched.
    #[track_caller]
    pub fn assert_nothing_dispatched(&self) {
        let mut counts: IndexMap<String, usize> = IndexMap::new();
        for name in self.dispatched_events() {
            *counts.entry(name).or_default() += 1;
        }
        let total: usize = counts.values().sum();
        let names = counts
            .iter()
            .map(|(name, count)| format!("{name} dispatched {count} {}", times(*count)))
            .collect::<Vec<_>>()
            .join("\n- ");
        assert!(
            total == 0,
            "{total} unexpected events were dispatched:\n\n- {names}\n"
        );
    }

    /// Assert that the listener `L` is attached to the event `E`.
    #[track_caller]
    pub fn assert_listening<E: 'static, L: 'static>(&self) {
        assert!(
            self.registry()
                .has_typed_listener(TypeId::of::<E>(), TypeId::of::<L>()),
            "Event [{}] does not have the [{}] listener attached to it",
            type_name::<E>(),
            type_name::<L>()
        );
    }

    /// Assert that the event `E` has at least one listener.
    #[track_caller]
    pub fn assert_has_listeners<E: 'static>(&self) {
        assert!(
            self.has_listeners::<E>(),
            "Event [{}] does not have any listeners attached to it",
            type_name::<E>()
        );
    }

    /// Assert that a named event has at least one listener (wildcards count).
    #[track_caller]
    pub fn assert_listening_named(&self, event: &str) {
        assert!(
            self.has_listeners_named(event),
            "Event [{event}] does not have any listeners attached to it"
        );
    }
}
