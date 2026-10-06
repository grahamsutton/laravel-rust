//! Small, composable behaviours shared across the framework.

/// Conditionally apply a callback to a value, fluently.
///
/// ```
/// use illuminate_support::{collect, Conditionable};
///
/// let sorted = true;
/// let items = collect(vec![3, 1, 2]).when(sorted, |c| c.sort());
/// assert_eq!(items.all(), &[1, 2, 3]);
/// ```
pub trait Conditionable: Sized {
    /// Apply the callback if the given condition is true.
    fn when(self, condition: bool, callback: impl FnOnce(Self) -> Self) -> Self {
        if condition { callback(self) } else { self }
    }

    /// Apply the callback if the given condition is true, or the default otherwise.
    fn when_else(
        self,
        condition: bool,
        callback: impl FnOnce(Self) -> Self,
        default: impl FnOnce(Self) -> Self,
    ) -> Self {
        if condition {
            callback(self)
        } else {
            default(self)
        }
    }

    /// Apply the callback with the contained value if the option is `Some`.
    fn when_some<V>(self, value: Option<V>, callback: impl FnOnce(Self, V) -> Self) -> Self {
        match value {
            Some(v) => callback(self, v),
            None => self,
        }
    }

    /// Apply the callback unless the given condition is true.
    fn unless(self, condition: bool, callback: impl FnOnce(Self) -> Self) -> Self {
        if condition { self } else { callback(self) }
    }
}

/// Call the given closure with a reference to the value, then return the value.
pub trait Tappable: Sized {
    fn tap(mut self, callback: impl FnOnce(&mut Self)) -> Self {
        callback(&mut self);
        self
    }

    /// Pass the value through the given callback and return the result.
    fn pipe<R>(self, callback: impl FnOnce(Self) -> R) -> R {
        callback(self)
    }
}
