//! Collections: a fluent, convenient wrapper for working with lists of data.
//!
//! ```
//! use illuminate_support::collect;
//!
//! let total = collect(vec![1, 2, 3, 4, 5])
//!     .filter(|n| n % 2 == 1)
//!     .map(|n| n * 10)
//!     .sum_by(|n| *n);
//!
//! assert_eq!(total, 90);
//! ```

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt::{self, Display};
use std::hash::Hash;
use std::ops::{Deref, DerefMut};

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::traits::{Conditionable, Tappable};
use crate::value::{Value, ValueExt, to_value};

pub mod lazy;
mod methods;

pub use lazy::LazyCollection;

/// A fluent wrapper around a list of items.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Collection<T> {
    items: Vec<T>,
}

impl<T> Default for Collection<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

/// Create a collection from the given items.
pub fn collect<T>(items: impl IntoIterator<Item = T>) -> Collection<T> {
    Collection::make(items)
}

impl<T> Collection<T> {
    /// Create a new, empty collection.
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Create a new collection from the given items.
    pub fn make(items: impl IntoIterator<Item = T>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    /// Create a collection by invoking the callback `n` times (1-indexed).
    pub fn times(n: usize, mut callback: impl FnMut(usize) -> T) -> Self {
        Self::make((1..=n).map(&mut callback))
    }

    /// Get all of the items in the collection.
    pub fn all(&self) -> &[T] {
        &self.items
    }

    /// Convert the collection into a plain vector.
    pub fn into_vec(self) -> Vec<T> {
        self.items
    }

    /// Count the number of items in the collection.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// Determine if the collection is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Determine if the collection is not empty.
    pub fn is_not_empty(&self) -> bool {
        !self.items.is_empty()
    }

    /// Get the first item in the collection.
    pub fn first(&self) -> Option<&T> {
        self.items.first()
    }

    /// Get the first item passing the given truth test.
    pub fn first_where_fn(&self, mut callback: impl FnMut(&T) -> bool) -> Option<&T> {
        self.items.iter().find(|item| callback(item))
    }

    /// Get the last item in the collection.
    pub fn last(&self) -> Option<&T> {
        self.items.last()
    }

    /// Get the item at the given index.
    pub fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index)
    }

    /// Push an item onto the end of the collection.
    pub fn push(mut self, item: T) -> Self {
        self.items.push(item);
        self
    }

    /// Push an item onto the beginning of the collection.
    pub fn prepend(mut self, item: T) -> Self {
        self.items.insert(0, item);
        self
    }

    /// Push an item onto the collection in place.
    pub fn add(&mut self, item: T) -> &mut Self {
        self.items.push(item);
        self
    }

    /// Remove and return the last item.
    pub fn pop(&mut self) -> Option<T> {
        self.items.pop()
    }

    /// Remove and return the first item.
    pub fn shift(&mut self) -> Option<T> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.items.remove(0))
        }
    }

    /// Run a map over each of the items.
    pub fn map<U>(self, callback: impl FnMut(T) -> U) -> Collection<U> {
        Collection {
            items: self.items.into_iter().map(callback).collect(),
        }
    }

    /// Run a map over each of the items, passing the index as well.
    pub fn map_with_index<U>(self, mut callback: impl FnMut(T, usize) -> U) -> Collection<U> {
        Collection {
            items: self
                .items
                .into_iter()
                .enumerate()
                .map(|(i, item)| callback(item, i))
                .collect(),
        }
    }

    /// Transform each item in the collection in place.
    pub fn transform(&mut self, mut callback: impl FnMut(&mut T)) -> &mut Self {
        for item in self.items.iter_mut() {
            callback(item);
        }
        self
    }

    /// Map each item into a collection and flatten the result by one level.
    pub fn flat_map<U, I: IntoIterator<Item = U>>(
        self,
        callback: impl FnMut(T) -> I,
    ) -> Collection<U> {
        Collection {
            items: self.items.into_iter().flat_map(callback).collect(),
        }
    }

    /// Run a filter over each of the items.
    pub fn filter(self, mut callback: impl FnMut(&T) -> bool) -> Self {
        Self {
            items: self.items.into_iter().filter(|i| callback(i)).collect(),
        }
    }

    /// Create a collection of all items that do not pass a given truth test.
    pub fn reject(self, mut callback: impl FnMut(&T) -> bool) -> Self {
        self.filter(|item| !callback(item))
    }

    /// Execute a callback over each item.
    pub fn each(&self, mut callback: impl FnMut(&T)) -> &Self {
        for item in &self.items {
            callback(item);
        }
        self
    }

    /// Execute a callback over each item until it returns `false`.
    pub fn each_while(&self, mut callback: impl FnMut(&T) -> bool) -> &Self {
        for item in &self.items {
            if !callback(item) {
                break;
            }
        }
        self
    }

    /// Determine if any item passes the given truth test.
    pub fn contains_fn(&self, callback: impl FnMut(&T) -> bool) -> bool {
        self.items.iter().any(callback)
    }

    /// Determine if all items pass the given truth test.
    pub fn every(&self, callback: impl FnMut(&T) -> bool) -> bool {
        self.items.iter().all(callback)
    }

    /// Partition the collection into two collections using the given callback.
    pub fn partition(self, mut callback: impl FnMut(&T) -> bool) -> (Self, Self) {
        let (pass, fail): (Vec<T>, Vec<T>) =
            self.items.into_iter().partition(|item| callback(item));
        (Self { items: pass }, Self { items: fail })
    }

    /// Reduce the collection to a single value.
    pub fn reduce<A>(self, initial: A, callback: impl FnMut(A, T) -> A) -> A {
        self.items.into_iter().fold(initial, callback)
    }

    /// Reverse the order of the items.
    pub fn reverse(mut self) -> Self {
        self.items.reverse();
        self
    }

    /// Take the first `limit` items (or the last `-limit` items when negative).
    pub fn take(self, limit: isize) -> Self {
        let len = self.items.len() as isize;
        if limit >= 0 {
            Self {
                items: self.items.into_iter().take(limit as usize).collect(),
            }
        } else {
            let skip = (len + limit).max(0) as usize;
            Self {
                items: self.items.into_iter().skip(skip).collect(),
            }
        }
    }

    /// Skip the first `count` items.
    pub fn skip(self, count: usize) -> Self {
        Self {
            items: self.items.into_iter().skip(count).collect(),
        }
    }

    /// Take items while the callback returns true.
    pub fn take_while(self, mut callback: impl FnMut(&T) -> bool) -> Self {
        Self {
            items: self.items.into_iter().take_while(|i| callback(i)).collect(),
        }
    }

    /// Take items until the callback returns true.
    pub fn take_until(self, mut callback: impl FnMut(&T) -> bool) -> Self {
        self.take_while(|i| !callback(i))
    }

    /// Skip items while the callback returns true.
    pub fn skip_while(self, mut callback: impl FnMut(&T) -> bool) -> Self {
        Self {
            items: self.items.into_iter().skip_while(|i| callback(i)).collect(),
        }
    }

    /// Skip items until the callback returns true.
    pub fn skip_until(self, mut callback: impl FnMut(&T) -> bool) -> Self {
        self.skip_while(|i| !callback(i))
    }

    /// Slice the underlying collection.
    pub fn slice(self, offset: usize, length: Option<usize>) -> Self {
        let iter = self.items.into_iter().skip(offset);
        Self {
            items: match length {
                Some(len) => iter.take(len).collect(),
                None => iter.collect(),
            },
        }
    }

    /// "Paginate" the collection by slicing it into a smaller collection.
    pub fn for_page(self, page: usize, per_page: usize) -> Self {
        let offset = page.saturating_sub(1) * per_page;
        self.slice(offset, Some(per_page))
    }

    /// Chunk the collection into collections of the given size.
    pub fn chunk(self, size: usize) -> Collection<Collection<T>> {
        let size = size.max(1);
        let mut chunks = Vec::new();
        let mut current = Vec::with_capacity(size);
        for item in self.items {
            current.push(item);
            if current.len() == size {
                chunks.push(Collection {
                    items: std::mem::replace(&mut current, Vec::with_capacity(size)),
                });
            }
        }
        if !current.is_empty() {
            chunks.push(Collection { items: current });
        }
        Collection { items: chunks }
    }

    /// Split the collection into the given number of groups.
    pub fn split(self, groups: usize) -> Collection<Collection<T>> {
        if self.items.is_empty() || groups == 0 {
            return Collection::new();
        }
        let len = self.items.len();
        let base = len / groups;
        let remainder = len % groups;
        let mut iter = self.items.into_iter();
        let mut out = Vec::new();
        for i in 0..groups {
            let size = base + usize::from(i < remainder);
            if size == 0 {
                continue;
            }
            out.push(Collection {
                items: iter.by_ref().take(size).collect(),
            });
        }
        Collection { items: out }
    }

    /// Sort the collection using the given comparison callback.
    pub fn sort_by_fn(mut self, compare: impl FnMut(&T, &T) -> Ordering) -> Self {
        self.items.sort_by(compare);
        self
    }

    /// Sort the collection by the given key.
    pub fn sort_by<K: Ord>(mut self, mut key: impl FnMut(&T) -> K) -> Self {
        self.items.sort_by_key(|a| key(a));
        self
    }

    /// Sort the collection in descending order by the given key.
    pub fn sort_by_desc<K: Ord>(mut self, mut key: impl FnMut(&T) -> K) -> Self {
        self.items.sort_by_key(|item| std::cmp::Reverse(key(item)));
        self
    }

    /// Group the collection's items by the given key.
    pub fn group_by<K: Hash + Eq>(
        self,
        mut key: impl FnMut(&T) -> K,
    ) -> IndexMap<K, Collection<T>> {
        let mut groups: IndexMap<K, Collection<T>> = IndexMap::new();
        for item in self.items {
            groups.entry(key(&item)).or_default().items.push(item);
        }
        groups
    }

    /// Key the collection by the given key (later items win).
    pub fn key_by<K: Hash + Eq>(self, mut key: impl FnMut(&T) -> K) -> IndexMap<K, T> {
        let mut map = IndexMap::new();
        for item in self.items {
            map.insert(key(&item), item);
        }
        map
    }

    /// Count the occurrences of each key.
    pub fn count_by<K: Hash + Eq>(&self, mut key: impl FnMut(&T) -> K) -> IndexMap<K, usize> {
        let mut map = IndexMap::new();
        for item in &self.items {
            *map.entry(key(item)).or_insert(0) += 1;
        }
        map
    }

    /// Return only unique items, using the given key to determine uniqueness.
    pub fn unique_by<K: Hash + Eq>(self, mut key: impl FnMut(&T) -> K) -> Self {
        let mut seen = HashSet::new();
        Self {
            items: self
                .items
                .into_iter()
                .filter(|item| seen.insert(key(item)))
                .collect(),
        }
    }

    /// Sum the values returned by the callback.
    pub fn sum_by<N: std::iter::Sum<N>>(&self, callback: impl FnMut(&T) -> N) -> N {
        self.items.iter().map(callback).sum()
    }

    /// Get the average of the values returned by the callback.
    pub fn avg_by(&self, mut callback: impl FnMut(&T) -> f64) -> Option<f64> {
        if self.items.is_empty() {
            return None;
        }
        let total: f64 = self.items.iter().map(&mut callback).sum();
        Some(total / self.items.len() as f64)
    }

    /// Get the minimum value returned by the callback.
    pub fn min_by<K: Ord>(&self, mut callback: impl FnMut(&T) -> K) -> Option<K> {
        self.items.iter().map(&mut callback).min()
    }

    /// Get the maximum value returned by the callback.
    pub fn max_by<K: Ord>(&self, mut callback: impl FnMut(&T) -> K) -> Option<K> {
        self.items.iter().map(&mut callback).max()
    }

    /// Search the collection for the first item passing the truth test,
    /// returning its index.
    pub fn search_fn(&self, callback: impl FnMut(&T) -> bool) -> Option<usize> {
        self.items.iter().position(callback)
    }

    /// Get the one and only item passing the truth test, or an error.
    pub fn sole_fn(self, mut callback: impl FnMut(&T) -> bool) -> crate::Result<T> {
        let mut matches: Vec<T> = self.items.into_iter().filter(|i| callback(i)).collect();
        match matches.len() {
            0 => Err(ItemNotFoundException.into()),
            1 => Ok(matches.remove(0)),
            n => Err(MultipleItemsFoundException { count: n }.into()),
        }
    }

    /// Zip the collection together with the given items.
    pub fn zip<U>(self, other: impl IntoIterator<Item = U>) -> Collection<(T, U)> {
        Collection {
            items: self.items.into_iter().zip(other).collect(),
        }
    }

    /// Merge the given items onto the end of the collection.
    pub fn merge(mut self, other: impl IntoIterator<Item = T>) -> Self {
        self.items.extend(other);
        self
    }

    /// Alias of `merge`, matching Laravel's `concat`.
    pub fn concat(self, other: impl IntoIterator<Item = T>) -> Self {
        self.merge(other)
    }

    /// Pass the collection to the given callback and return the result.
    pub fn pipe<R>(self, callback: impl FnOnce(Self) -> R) -> R {
        callback(self)
    }

    /// Get a lazy-ish iterator over references to the items.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    /// Get an iterator of mutable references.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.items.iter_mut()
    }

    /// Collect the items into a different collection type.
    pub fn collect<C: FromIterator<T>>(self) -> C {
        self.items.into_iter().collect()
    }
}

impl<T: Clone> Collection<T> {
    /// Get the first item, or the default.
    pub fn first_or(&self, default: T) -> T {
        self.items.first().cloned().unwrap_or(default)
    }

    /// Create a new collection with the given item pushed onto a copy.
    pub fn with(&self, item: T) -> Self {
        let mut items = self.items.clone();
        items.push(item);
        Self { items }
    }

    /// Return the items at the given indexes ("nth" from offset).
    pub fn nth(&self, step: usize, offset: usize) -> Self {
        Self {
            items: self
                .items
                .iter()
                .skip(offset)
                .step_by(step.max(1))
                .cloned()
                .collect(),
        }
    }

    /// Retrieve random items from the collection.
    pub fn random(&self, count: usize) -> Self {
        use rand::seq::SliceRandom;
        let mut items = self.items.clone();
        items.shuffle(&mut rand::rng());
        items.truncate(count);
        Self { items }
    }

    /// Shuffle the items in the collection.
    pub fn shuffle(mut self) -> Self {
        use rand::seq::SliceRandom;
        self.items.shuffle(&mut rand::rng());
        self
    }

    /// Pad the collection to the given size with a value.
    pub fn pad(mut self, size: isize, value: T) -> Self {
        let target = size.unsigned_abs();
        if self.items.len() >= target {
            return self;
        }
        let padding = vec![value; target - self.items.len()];
        if size > 0 {
            self.items.extend(padding);
            self
        } else {
            let mut items = padding;
            items.extend(self.items);
            Self { items }
        }
    }

    /// Create a collection by sliding a window over the items.
    pub fn sliding(&self, size: usize, step: usize) -> Collection<Collection<T>> {
        if size == 0 || self.items.len() < size {
            return Collection::new();
        }
        let mut out = Vec::new();
        let mut start = 0;
        while start + size <= self.items.len() {
            out.push(Collection::make(self.items[start..start + size].to_vec()));
            start += step.max(1);
        }
        Collection { items: out }
    }
}

impl<T: PartialEq> Collection<T> {
    /// Determine if the collection contains the given item.
    pub fn contains(&self, item: &T) -> bool {
        self.items.contains(item)
    }

    /// Determine if the collection does not contain the given item.
    pub fn doesnt_contain(&self, item: &T) -> bool {
        !self.contains(item)
    }

    /// Search the collection for a given value and return its index.
    pub fn search(&self, item: &T) -> Option<usize> {
        self.items.iter().position(|i| i == item)
    }

    /// Get the items in the collection that are not present in the given items.
    pub fn diff(self, other: impl IntoIterator<Item = T>) -> Self {
        let other: Vec<T> = other.into_iter().collect();
        self.filter(|item| !other.contains(item))
    }

    /// Intersect the collection with the given items.
    pub fn intersect(self, other: impl IntoIterator<Item = T>) -> Self {
        let other: Vec<T> = other.into_iter().collect();
        self.filter(|item| other.contains(item))
    }

    /// Return only unique items (by equality), preserving order.
    pub fn unique(self) -> Self {
        let mut out: Vec<T> = Vec::with_capacity(self.items.len());
        for item in self.items {
            if !out.contains(&item) {
                out.push(item);
            }
        }
        Self { items: out }
    }

    /// Get the items that appear more than once.
    pub fn duplicates(&self) -> Self
    where
        T: Clone,
    {
        let mut seen: Vec<&T> = Vec::new();
        let mut dupes: Vec<T> = Vec::new();
        for item in &self.items {
            if seen.contains(&item) {
                if !dupes.contains(item) {
                    dupes.push(item.clone());
                }
            } else {
                seen.push(item);
            }
        }
        Self { items: dupes }
    }
}

impl<T: Ord> Collection<T> {
    /// Sort the items in ascending order.
    pub fn sort(mut self) -> Self {
        self.items.sort();
        self
    }

    /// Sort the items in descending order.
    pub fn sort_desc(mut self) -> Self {
        self.items.sort_by(|a, b| b.cmp(a));
        self
    }

    /// Get the minimum item.
    pub fn min(&self) -> Option<&T> {
        self.items.iter().min()
    }

    /// Get the maximum item.
    pub fn max(&self) -> Option<&T> {
        self.items.iter().max()
    }
}

impl<T: Copy + std::iter::Sum<T>> Collection<T> {
    /// Get the sum of the items.
    pub fn sum(&self) -> T {
        self.items.iter().copied().sum()
    }
}

impl<T: Copy + Into<f64>> Collection<T> {
    /// Get the average of the items.
    pub fn avg(&self) -> Option<f64> {
        self.avg_by(|item| (*item).into())
    }

    /// Alias of `avg`.
    pub fn average(&self) -> Option<f64> {
        self.avg()
    }

    /// Get the median of the items.
    pub fn median(&self) -> Option<f64> {
        if self.items.is_empty() {
            return None;
        }
        let mut values: Vec<f64> = self.items.iter().map(|i| (*i).into()).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        let mid = values.len() / 2;
        Some(if values.len().is_multiple_of(2) {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        })
    }
}

impl<T: Display> Collection<T> {
    /// Concatenate the items into a string using the given glue.
    pub fn implode(&self, glue: &str) -> String {
        self.items
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
            .join(glue)
    }

    /// Join the items with a glue, using a different glue for the final item.
    pub fn join(&self, glue: &str, final_glue: &str) -> String {
        let strings: Vec<String> = self.items.iter().map(|i| i.to_string()).collect();
        match strings.len() {
            0 => String::new(),
            1 => strings[0].clone(),
            n => format!("{}{}{}", strings[..n - 1].join(glue), final_glue, strings[n - 1]),
        }
    }
}

impl<T> Collection<Collection<T>> {
    /// Collapse a collection of collections into a single, flat collection.
    pub fn collapse(self) -> Collection<T> {
        Collection {
            items: self.items.into_iter().flat_map(|c| c.items).collect(),
        }
    }
}

impl<T> Collection<Vec<T>> {
    /// Flatten a collection of vectors into a single collection.
    pub fn flatten(self) -> Collection<T> {
        Collection {
            items: self.items.into_iter().flatten().collect(),
        }
    }
}

impl<T> Collection<Option<T>> {
    /// Remove all `None` values, unwrapping the rest.
    pub fn filter_some(self) -> Collection<T> {
        Collection {
            items: self.items.into_iter().flatten().collect(),
        }
    }
}

/// Methods that inspect items through their serialized representation.
///
/// This is how "dot notation" access (`pluck("user.name")`,
/// `where_("active", true)`) works for any serializable item, including
/// Eloquent models and plain JSON values.
impl<T: Serialize> Collection<T> {
    /// Get the values of the given key from each item.
    pub fn pluck(&self, key: &str) -> Collection<Value> {
        Collection {
            items: self
                .items
                .iter()
                .map(|item| to_value(item).dot_or_null(key))
                .collect(),
        }
    }

    /// Get the values of the given key from each item, deserialized into `V`.
    pub fn pluck_as<V: DeserializeOwned>(&self, key: &str) -> Collection<V> {
        Collection {
            items: self
                .items
                .iter()
                .filter_map(|item| crate::value::cast(to_value(item).dot_or_null(key)).ok())
                .collect(),
        }
    }

    /// Filter items where the given key loosely equals the given value.
    pub fn where_(self, key: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.filter(|item| loose_eq(&to_value(item).dot_or_null(key), &value))
    }

    /// Filter items by comparing the given key using an operator.
    pub fn where_op(self, key: &str, operator: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.filter(|item| compare_values(&to_value(item).dot_or_null(key), operator, &value))
    }

    /// Filter items where the given key is contained in the given values.
    pub fn where_in(self, key: &str, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        self.filter(|item| {
            let v = to_value(item).dot_or_null(key);
            values.iter().any(|candidate| loose_eq(&v, candidate))
        })
    }

    /// Filter items where the given key is not contained in the given values.
    pub fn where_not_in(
        self,
        key: &str,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        self.filter(|item| {
            let v = to_value(item).dot_or_null(key);
            !values.iter().any(|candidate| loose_eq(&v, candidate))
        })
    }

    /// Filter items where the given key is null.
    pub fn where_null(self, key: &str) -> Self {
        self.filter(|item| to_value(item).dot_or_null(key).is_null())
    }

    /// Filter items where the given key is not null.
    pub fn where_not_null(self, key: &str) -> Self {
        self.filter(|item| !to_value(item).dot_or_null(key).is_null())
    }

    /// Get the first item where the given key loosely equals the value.
    pub fn first_where(&self, key: &str, value: impl Into<Value>) -> Option<&T> {
        let value = value.into();
        self.items
            .iter()
            .find(|item| loose_eq(&to_value(*item).dot_or_null(key), &value))
    }

    /// Sort the collection by the value of the given key.
    pub fn sort_by_key(self, key: &str) -> Self {
        self.sort_by_fn(|a, b| compare_for_sort(&to_value(a).dot_or_null(key), &to_value(b).dot_or_null(key)))
    }

    /// Sort the collection by the value of the given key, descending.
    pub fn sort_by_key_desc(self, key: &str) -> Self {
        self.sort_by_fn(|a, b| compare_for_sort(&to_value(b).dot_or_null(key), &to_value(a).dot_or_null(key)))
    }

    /// Group the items by the string value of the given key.
    pub fn group_by_key(self, key: &str) -> IndexMap<String, Collection<T>> {
        self.group_by(|item| to_value(item).dot_or_null(key).to_string_lossy())
    }

    /// Key the items by the string value of the given key.
    pub fn key_by_key(self, key: &str) -> IndexMap<String, T> {
        self.key_by(|item| to_value(item).dot_or_null(key).to_string_lossy())
    }

    /// Sum the numeric values of the given key.
    pub fn sum_key(&self, key: &str) -> f64 {
        self.items
            .iter()
            .filter_map(|item| to_value(item).dot_or_null(key).to_f64_lossy())
            .sum()
    }

    /// Get the average of the numeric values of the given key.
    pub fn avg_key(&self, key: &str) -> Option<f64> {
        let values: Vec<f64> = self
            .items
            .iter()
            .filter_map(|item| to_value(item).dot_or_null(key).to_f64_lossy())
            .collect();
        if values.is_empty() {
            None
        } else {
            Some(values.iter().sum::<f64>() / values.len() as f64)
        }
    }

    /// Get the maximum numeric value of the given key.
    pub fn max_key(&self, key: &str) -> Option<f64> {
        self.items
            .iter()
            .filter_map(|item| to_value(item).dot_or_null(key).to_f64_lossy())
            .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))))
    }

    /// Get the minimum numeric value of the given key.
    pub fn min_key(&self, key: &str) -> Option<f64> {
        self.items
            .iter()
            .filter_map(|item| to_value(item).dot_or_null(key).to_f64_lossy())
            .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.min(v))))
    }

    /// Convert the collection into a plain value (an array).
    pub fn to_array(&self) -> Value {
        to_value(&self.items)
    }

    /// Get the collection of items as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.items).unwrap_or_else(|_| "[]".to_string())
    }

    /// Get the collection of items as pretty-printed JSON.
    pub fn to_pretty_json(&self) -> String {
        serde_json::to_string_pretty(&self.items).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Loosely compare two values the way PHP's `==` would.
pub fn loose_eq(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Value::Number(_), _) | (_, Value::Number(_)) => match (a.to_f64_lossy(), b.to_f64_lossy()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        (Value::Bool(x), other) | (other, Value::Bool(x)) => *x == other.truthy(),
        (Value::Null, other) | (other, Value::Null) => !other.truthy(),
        _ => a.to_string_lossy() == b.to_string_lossy(),
    }
}

/// Compare two values using the given operator (`=`, `!=`, `<`, `>=`, ...).
pub fn compare_values(left: &Value, operator: &str, right: &Value) -> bool {
    match operator {
        "=" | "==" => loose_eq(left, right),
        "!=" | "<>" => !loose_eq(left, right),
        "===" => left == right,
        "!==" => left != right,
        "<" => compare_for_sort(left, right) == Ordering::Less,
        "<=" => compare_for_sort(left, right) != Ordering::Greater,
        ">" => compare_for_sort(left, right) == Ordering::Greater,
        ">=" => compare_for_sort(left, right) != Ordering::Less,
        _ => loose_eq(left, right),
    }
}

/// Order two values: numbers numerically, everything else as strings.
pub fn compare_for_sort(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        _ => match (a.to_f64_lossy(), b.to_f64_lossy()) {
            (Some(x), Some(y)) if !matches!(a, Value::String(_)) || !matches!(b, Value::String(_)) || (a.as_str().map(|s| s.trim().parse::<f64>().is_ok()).unwrap_or(false) && b.as_str().map(|s| s.trim().parse::<f64>().is_ok()).unwrap_or(false)) => {
                x.partial_cmp(&y).unwrap_or(Ordering::Equal)
            }
            _ => a.to_string_lossy().cmp(&b.to_string_lossy()),
        },
    }
}

/// Thrown when `sole` finds no matching item.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Item not found.")]
pub struct ItemNotFoundException;

/// Thrown when `sole` finds more than one matching item.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{count} items were found.")]
pub struct MultipleItemsFoundException {
    pub count: usize,
}

impl<T> Conditionable for Collection<T> {}
impl<T> Tappable for Collection<T> {}

impl<T> Deref for Collection<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.items
    }
}

impl<T> DerefMut for Collection<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.items
    }
}

impl<T: fmt::Debug> fmt::Debug for Collection<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.items.iter()).finish()
    }
}

impl<T> From<Vec<T>> for Collection<T> {
    fn from(items: Vec<T>) -> Self {
        Self { items }
    }
}

impl<T> From<Collection<T>> for Vec<T> {
    fn from(collection: Collection<T>) -> Self {
        collection.items
    }
}

impl<T: Clone> From<&[T]> for Collection<T> {
    fn from(items: &[T]) -> Self {
        Self {
            items: items.to_vec(),
        }
    }
}

impl<T, const N: usize> From<[T; N]> for Collection<T> {
    fn from(items: [T; N]) -> Self {
        Self {
            items: items.into(),
        }
    }
}

impl<T> FromIterator<T> for Collection<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self {
            items: iter.into_iter().collect(),
        }
    }
}

impl<T> Extend<T> for Collection<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.items.extend(iter);
    }
}

impl<T> IntoIterator for Collection<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a Collection<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Collection<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter_mut()
    }
}

impl<T: Serialize> Serialize for Collection<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.items.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Collection<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            items: Vec::<T>::deserialize(deserializer)?,
        })
    }
}

impl<T> From<Collection<T>> for Value
where
    T: Serialize,
{
    fn from(collection: Collection<T>) -> Self {
        to_value(&collection.items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_maps_filters_and_reduces() {
        let c = collect(vec![1, 2, 3, 4]);
        assert_eq!(c.clone().map(|n| n * 2).into_vec(), vec![2, 4, 6, 8]);
        assert_eq!(c.clone().filter(|n| *n > 2).into_vec(), vec![3, 4]);
        assert_eq!(c.clone().reject(|n| *n > 2).into_vec(), vec![1, 2]);
        assert_eq!(c.clone().reduce(0, |carry, n| carry + n), 10);
        assert_eq!(c.sum(), 10);
    }

    #[test]
    fn it_chunks_and_splits() {
        let chunks = collect(1..=7).chunk(3);
        assert_eq!(chunks.count(), 3);
        assert_eq!(chunks.last().unwrap().all(), &[7]);

        let groups = collect(1..=10).split(3);
        assert_eq!(groups[0].all(), &[1, 2, 3, 4]);
        assert_eq!(groups[2].all(), &[8, 9, 10]);
    }

    #[test]
    fn it_works_with_values_using_dot_notation() {
        let users = collect(vec![
            json!({"name": "Taylor", "role": {"name": "admin"}, "votes": 10}),
            json!({"name": "Abigail", "role": {"name": "user"}, "votes": 5}),
            json!({"name": "James", "role": {"name": "admin"}, "votes": 20}),
        ]);

        assert_eq!(
            users.pluck("name").into_vec(),
            vec![json!("Taylor"), json!("Abigail"), json!("James")]
        );
        assert_eq!(users.clone().where_("role.name", "admin").count(), 2);
        assert_eq!(users.clone().where_op("votes", ">", 8).count(), 2);
        assert_eq!(users.sum_key("votes"), 35.0);
        assert_eq!(
            users.clone().sort_by_key_desc("votes").pluck("name").first(),
            Some(&json!("James"))
        );
        assert_eq!(users.group_by_key("role.name")["admin"].count(), 2);
    }

    #[test]
    fn it_implodes_and_joins() {
        let c = collect(vec!["a", "b", "c"]);
        assert_eq!(c.implode(", "), "a, b, c");
        assert_eq!(c.join(", ", " and "), "a, b and c");
    }

    #[test]
    fn it_takes_from_either_end() {
        assert_eq!(collect(1..=5).take(2).into_vec(), vec![1, 2]);
        assert_eq!(collect(1..=5).take(-2).into_vec(), vec![4, 5]);
    }

    #[test]
    fn sole_returns_exactly_one_item() {
        assert_eq!(collect(vec![1, 2, 3]).sole_fn(|n| *n == 2).unwrap(), 2);
        assert!(collect(vec![1, 2, 3]).sole_fn(|n| *n > 1).is_err());
        assert!(collect(vec![1, 2, 3]).sole_fn(|n| *n > 5).is_err());
    }
}
