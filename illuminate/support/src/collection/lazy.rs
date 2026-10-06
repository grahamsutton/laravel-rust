//! Lazy collections: the collection API over an iterator, so huge (or
//! infinite) data sets can be processed one item at a time.
//!
//! ```
//! use illuminate_support::LazyCollection;
//!
//! let squares = LazyCollection::times(usize::MAX, |n| n * n)
//!     .filter(|n| n % 2 == 1)
//!     .take(3)
//!     .collect();
//!
//! assert_eq!(squares.all(), &[1, 9, 25]);
//! ```

use serde::Serialize;

use super::{Collection, compare_values, loose_eq};
use crate::carbon::Carbon;
use crate::value::{Value, ValueExt, to_value};

/// A lazily evaluated collection backed by an iterator.
pub struct LazyCollection<'a, T> {
    iter: Box<dyn Iterator<Item = T> + 'a>,
}

impl<'a, T: 'a> LazyCollection<'a, T> {
    /// Create a lazy collection from anything iterable.
    pub fn make<I>(items: I) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: 'a,
    {
        Self {
            iter: Box::new(items.into_iter()),
        }
    }

    /// Create a lazy collection from a generator-like closure that returns
    /// `None` when it is exhausted.
    pub fn from_fn(generator: impl FnMut() -> Option<T> + 'a) -> Self {
        Self::make(std::iter::from_fn(generator))
    }

    /// Create a lazy collection by invoking the callback `n` times (1-indexed).
    pub fn times(n: usize, callback: impl FnMut(usize) -> T + 'a) -> Self {
        Self::make((1..=n).map(callback))
    }

    /// Run a map over each of the items.
    pub fn map<U: 'a>(self, callback: impl FnMut(T) -> U + 'a) -> LazyCollection<'a, U> {
        LazyCollection::make(self.iter.map(callback))
    }

    /// Map each item into an iterable and flatten the result.
    pub fn flat_map<U: 'a, I>(self, callback: impl FnMut(T) -> I + 'a) -> LazyCollection<'a, U>
    where
        I: IntoIterator<Item = U> + 'a,
    {
        LazyCollection::make(self.iter.flat_map(callback))
    }

    /// Keep the items passing the truth test.
    pub fn filter(self, mut callback: impl FnMut(&T) -> bool + 'a) -> Self {
        Self::make(self.iter.filter(move |item| callback(item)))
    }

    /// Remove the items passing the truth test.
    pub fn reject(self, mut callback: impl FnMut(&T) -> bool + 'a) -> Self {
        self.filter(move |item| !callback(item))
    }

    /// Take the first `limit` items.
    pub fn take(self, limit: usize) -> Self {
        Self::make(self.iter.take(limit))
    }

    /// Skip the first `count` items.
    pub fn skip(self, count: usize) -> Self {
        Self::make(self.iter.skip(count))
    }

    /// Take items while the callback returns true.
    pub fn take_while(self, mut callback: impl FnMut(&T) -> bool + 'a) -> Self {
        Self::make(self.iter.take_while(move |item| callback(item)))
    }

    /// Take items until the callback returns true.
    pub fn take_until(self, mut callback: impl FnMut(&T) -> bool + 'a) -> Self {
        self.take_while(move |item| !callback(item))
    }

    /// Take items until the given moment has passed.
    pub fn take_until_timeout(self, timeout: Carbon) -> Self {
        self.take_while(move |_| Carbon::now() < timeout)
    }

    /// Skip items while the callback returns true.
    pub fn skip_while(self, mut callback: impl FnMut(&T) -> bool + 'a) -> Self {
        Self::make(self.iter.skip_while(move |item| callback(item)))
    }

    /// Skip items until the callback returns true.
    pub fn skip_until(self, mut callback: impl FnMut(&T) -> bool + 'a) -> Self {
        self.skip_while(move |item| !callback(item))
    }

    /// Chunk the items into collections of the given size.
    pub fn chunk(self, size: usize) -> LazyCollection<'a, Collection<T>> {
        let size = size.max(1);
        let mut iter = self.iter;
        LazyCollection::from_fn(move || {
            let chunk: Vec<T> = iter.by_ref().take(size).collect();
            (!chunk.is_empty()).then(|| Collection::make(chunk))
        })
    }

    /// Call the callback with each item as it is enumerated.
    pub fn tap_each(self, mut callback: impl FnMut(&T) + 'a) -> Self {
        Self::make(self.iter.inspect(move |item| callback(item)))
    }

    /// Zip the items together with another iterable.
    pub fn zip<U: 'a, I>(self, other: I) -> LazyCollection<'a, (T, U)>
    where
        I: IntoIterator<Item = U>,
        I::IntoIter: 'a,
    {
        LazyCollection::make(self.iter.zip(other))
    }

    /// Execute a callback over each item.
    pub fn each(self, callback: impl FnMut(T)) {
        self.iter.for_each(callback);
    }

    /// Execute a callback over each item until it returns `false`.
    pub fn each_while(self, mut callback: impl FnMut(T) -> bool) {
        for item in self.iter {
            if !callback(item) {
                break;
            }
        }
    }

    /// Reduce the items to a single value.
    pub fn reduce<A>(self, initial: A, callback: impl FnMut(A, T) -> A) -> A {
        self.iter.fold(initial, callback)
    }

    /// Determine if any item passes the truth test.
    pub fn contains_fn(mut self, mut callback: impl FnMut(&T) -> bool) -> bool {
        self.iter.any(|item| callback(&item))
    }

    /// Determine if every item passes the truth test.
    pub fn every(mut self, mut callback: impl FnMut(&T) -> bool) -> bool {
        self.iter.all(|item| callback(&item))
    }

    /// Get the first item.
    pub fn first(mut self) -> Option<T> {
        self.iter.next()
    }

    /// Get the first item passing the truth test.
    pub fn first_where_fn(mut self, mut callback: impl FnMut(&T) -> bool) -> Option<T> {
        self.iter.find(|item| callback(item))
    }

    /// Count the items (enumerating them).
    pub fn count(self) -> usize {
        self.iter.count()
    }

    /// Sum the values returned by the callback.
    pub fn sum_by<N: std::iter::Sum<N>>(self, callback: impl FnMut(T) -> N) -> N {
        self.iter.map(callback).sum()
    }

    /// Collect the items into an eager [`Collection`].
    pub fn collect(self) -> Collection<T> {
        Collection::make(self.iter)
    }

    /// Collect the items into a vector.
    pub fn all(self) -> Vec<T> {
        self.iter.collect()
    }
}

impl<'a, T: Serialize + 'a> LazyCollection<'a, T> {
    /// Filter items where the given key loosely equals the given value.
    pub fn where_(self, key: &'a str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.filter(move |item| loose_eq(&to_value(item).dot_or_null(key), &value))
    }

    /// Filter items by comparing the given key using an operator.
    pub fn where_op(self, key: &'a str, operator: &'a str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.filter(move |item| compare_values(&to_value(item).dot_or_null(key), operator, &value))
    }

    /// Get the values of the given key from each item.
    pub fn pluck(self, key: &'a str) -> LazyCollection<'a, Value> {
        self.map(move |item| to_value(&item).dot_or_null(key))
    }
}

impl<'a> LazyCollection<'a, i64> {
    /// Create a lazy collection over the given (inclusive) range of integers.
    pub fn range(from: i64, to: i64) -> Self {
        if from <= to {
            Self::make(from..=to)
        } else {
            Self::make((to..=from).rev())
        }
    }
}

impl<T> Iterator for LazyCollection<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> std::fmt::Debug for LazyCollection<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LazyCollection { .. }")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect;
    use serde_json::json;
    use std::cell::Cell;

    #[test]
    fn it_is_lazy() {
        let enumerated = Cell::new(0);
        let first_two = LazyCollection::times(1_000_000, |n| n)
            .tap_each(|_| enumerated.set(enumerated.get() + 1))
            .take(2)
            .collect();
        assert_eq!(first_two.all(), &[1, 2]);
        assert_eq!(enumerated.get(), 2);
    }

    #[test]
    fn it_chunks_maps_and_filters() {
        let chunks = LazyCollection::range(1, 7).chunk(3).map(|c| c.into_vec()).all();
        assert_eq!(chunks, vec![vec![1, 2, 3], vec![4, 5, 6], vec![7]]);
        assert_eq!(LazyCollection::range(3, 1).all(), vec![3, 2, 1]);
        assert_eq!(collect(vec![1, 2, 3, 4]).lazy().reject(|n| n % 2 == 0).all(), vec![1, 3]);
        assert_eq!(LazyCollection::make(vec![1, 2, 3, 4]).skip_until(|n| *n >= 3).all(), vec![3, 4]);
        assert_eq!(LazyCollection::make(vec![1, 2, 3, 4]).take_until(|n| *n >= 3).all(), vec![1, 2]);
        assert_eq!(LazyCollection::make(vec![1, 2, 3]).sum_by(|n| n), 6);
        assert_eq!(LazyCollection::make(vec![1, 2, 3]).first(), Some(1));
        assert!(LazyCollection::make(vec![1, 2, 3]).every(|n| *n > 0));
        let mut counter = 0;
        let generated = LazyCollection::from_fn(|| {
            counter += 1;
            (counter <= 3).then_some(counter)
        });
        assert_eq!(generated.all(), vec![1, 2, 3]);
        let past = Carbon::now().sub_seconds(1);
        assert_eq!(LazyCollection::range(1, 10).take_until_timeout(past).count(), 0);
    }

    #[test]
    fn it_filters_serializable_items() {
        let users = vec![
            json!({"country": "FR", "balance": 200}),
            json!({"country": "FR", "balance": 50}),
            json!({"country": "US", "balance": 500}),
        ];
        let count = LazyCollection::make(users.clone())
            .where_("country", "FR")
            .where_op("balance", ">", "100")
            .count();
        assert_eq!(count, 1);
        assert_eq!(LazyCollection::make(users).pluck("country").all(), vec![json!("FR"), json!("FR"), json!("US")]);
    }
}
