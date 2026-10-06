//! Memoize a callback's result per call site.
//!
//! ```
//! use illuminate_support::once;
//!
//! fn random_number() -> u32 {
//!     once(|| rand::random::<u32>())
//! }
//!
//! assert_eq!(random_number(), random_number());
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::panic::Location;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

type Key = (&'static str, u32, u32, TypeId);

static CACHE: LazyLock<Mutex<HashMap<Key, Box<dyn Any + Send + Sync>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Run the callback once and cache its result for this call site.
///
/// The cached value is shared by every caller of the same source location
/// (for the life of the process, or until [`Once::flush`] is called).
#[track_caller]
pub fn once<T: Clone + Send + Sync + 'static>(callback: impl FnOnce() -> T) -> T {
    if !ENABLED.load(Ordering::SeqCst) {
        return callback();
    }
    let location = Location::caller();
    let key = (location.file(), location.line(), location.column(), TypeId::of::<T>());
    if let Some(value) = CACHE
        .lock()
        .unwrap()
        .get(&key)
        .and_then(|value| value.downcast_ref::<T>())
    {
        return value.clone();
    }
    let value = callback();
    CACHE
        .lock()
        .unwrap()
        .entry(key)
        .or_insert_with(|| Box::new(value.clone()));
    value
}

/// Controls for the [`once`] cache.
pub struct Once;

impl Once {
    /// Forget every memoized value.
    pub fn flush() {
        CACHE.lock().unwrap().clear();
    }

    /// Stop memoizing: `once` will run its callback every time.
    pub fn disable() {
        ENABLED.store(false, Ordering::SeqCst);
    }

    /// Resume memoizing.
    pub fn enable() {
        ENABLED.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    fn expensive() -> usize {
        once(|| CALLS.fetch_add(1, Ordering::SeqCst) + 100)
    }

    #[test]
    fn it_memoizes_per_call_site() {
        let first = expensive();
        assert_eq!(expensive(), first);
        assert_eq!(expensive(), first);
        let other = once(|| "a different call site");
        assert_eq!(other, "a different call site");
    }
}
