//! The rest of Laravel's collection methods.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{
    Collection, ItemNotFoundException, compare_for_sort, compare_values, loose_eq,
};
use crate::collection::lazy::LazyCollection;
use crate::value::{Map, Value, ValueExt, to_value};

impl<T> Collection<T> {
    /// Get the item after the first item passing the truth test.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// assert_eq!(collect(vec![2, 4, 6, 8]).after_fn(|n| *n > 5), Some(&8));
    /// ```
    pub fn after_fn(&self, mut callback: impl FnMut(&T) -> bool) -> Option<&T> {
        let index = self.items.iter().position(&mut callback)?;
        self.items.get(index + 1)
    }

    /// Get the item before the first item passing the truth test.
    pub fn before_fn(&self, mut callback: impl FnMut(&T) -> bool) -> Option<&T> {
        let index = self.items.iter().position(&mut callback)?;
        index.checked_sub(1).and_then(|i| self.items.get(i))
    }

    /// Chunk the collection into consecutive groups that share the same key.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let chunks = collect(vec![1, 1, 2, 2, 1]).chunk_by(|n| *n);
    /// assert_eq!(chunks.map(|c| c.into_vec()).into_vec(), vec![vec![1, 1], vec![2, 2], vec![1]]);
    /// ```
    pub fn chunk_by<K: PartialEq>(self, mut key: impl FnMut(&T) -> K) -> Collection<Collection<T>> {
        let mut chunks: Vec<Collection<T>> = Vec::new();
        let mut current_key: Option<K> = None;
        for item in self.items {
            let k = key(&item);
            match (&current_key, chunks.last_mut()) {
                (Some(previous), Some(chunk)) if *previous == k => chunk.items.push(item),
                _ => chunks.push(Collection { items: vec![item] }),
            }
            current_key = Some(k);
        }
        Collection { items: chunks }
    }

    /// Chunk the collection while the callback (given the item and the
    /// current chunk) returns true.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let chunks = collect("AABBCCCD".chars()).chunk_while(|c, chunk| Some(c) == chunk.last());
    /// let chunks: Vec<String> = chunks.map(|c| c.into_iter().collect()).into_vec();
    /// assert_eq!(chunks, vec!["AA", "BB", "CCC", "D"]);
    /// ```
    pub fn chunk_while(self, mut callback: impl FnMut(&T, &[T]) -> bool) -> Collection<Collection<T>> {
        let mut chunks: Vec<Collection<T>> = Vec::new();
        for item in self.items {
            match chunks.last_mut() {
                Some(chunk) if callback(&item, &chunk.items) => chunk.items.push(item),
                _ => chunks.push(Collection { items: vec![item] }),
            }
        }
        Collection { items: chunks }
    }

    /// Determine if the collection contains exactly one item.
    pub fn contains_one_item(&self) -> bool {
        self.items.len() == 1
    }

    /// Determine if the collection contains exactly one item (alias of `contains_one_item`).
    pub fn has_sole(&self) -> bool {
        self.contains_one_item()
    }

    /// Determine if exactly one item passes the truth test.
    pub fn has_sole_fn(&self, mut callback: impl FnMut(&T) -> bool) -> bool {
        self.items.iter().filter(|item| callback(item)).count() == 1
    }

    /// Determine if the collection contains more than one item.
    pub fn has_many(&self) -> bool {
        self.items.len() > 1
    }

    /// Determine if more than one item passes the truth test.
    pub fn has_many_fn(&self, mut callback: impl FnMut(&T) -> bool) -> bool {
        self.items.iter().filter(|item| callback(item)).count() > 1
    }

    /// Determine if no item passes the given truth test.
    pub fn doesnt_contain_fn(&self, callback: impl FnMut(&T) -> bool) -> bool {
        !self.contains_fn(callback)
    }

    /// Determine if any item passes the given truth test (alias of `contains_fn`).
    pub fn some(&self, callback: impl FnMut(&T) -> bool) -> bool {
        self.contains_fn(callback)
    }

    /// Get the items in the collection that are not present in the given
    /// items, using the comparator to decide equality.
    pub fn diff_using(
        self,
        other: impl IntoIterator<Item = T>,
        mut compare: impl FnMut(&T, &T) -> Ordering,
    ) -> Self {
        let other: Vec<T> = other.into_iter().collect();
        self.filter(|item| !other.iter().any(|o| compare(item, o) == Ordering::Equal))
    }

    /// Intersect the collection with the given items, using the comparator
    /// to decide equality.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let intersect = collect(vec!["Desk", "Sofa", "Chair"])
    ///     .intersect_using(vec!["desk", "chair", "bookcase"], |a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    /// assert_eq!(intersect.all(), &["Desk", "Chair"]);
    /// ```
    pub fn intersect_using(
        self,
        other: impl IntoIterator<Item = T>,
        mut compare: impl FnMut(&T, &T) -> Ordering,
    ) -> Self {
        let other: Vec<T> = other.into_iter().collect();
        self.filter(|item| other.iter().any(|o| compare(item, o) == Ordering::Equal))
    }

    /// Execute a callback over each item, along with its index.
    pub fn each_with_index(&self, mut callback: impl FnMut(&T, usize)) -> &Self {
        for (i, item) in self.items.iter().enumerate() {
            callback(item, i);
        }
        self
    }

    /// Get all items except those at the given indexes.
    pub fn except(self, indexes: &[usize]) -> Self {
        Self {
            items: self
                .items
                .into_iter()
                .enumerate()
                .filter(|(i, _)| !indexes.contains(i))
                .map(|(_, item)| item)
                .collect(),
        }
    }

    /// Get only the items at the given indexes.
    pub fn only(self, indexes: &[usize]) -> Self {
        Self {
            items: self
                .items
                .into_iter()
                .enumerate()
                .filter(|(i, _)| indexes.contains(i))
                .map(|(_, item)| item)
                .collect(),
        }
    }

    /// Get the first item, or fail with an [`ItemNotFoundException`].
    pub fn first_or_fail(&self) -> crate::Result<&T> {
        self.items.first().ok_or_else(|| ItemNotFoundException.into())
    }

    /// Get the first item passing the truth test, or fail with an
    /// [`ItemNotFoundException`].
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// assert_eq!(collect(vec![1, 2, 3, 4]).first_or_fail_fn(|n| *n > 2).unwrap(), &3);
    /// assert!(collect(vec![1, 2, 3, 4]).first_or_fail_fn(|n| *n > 5).is_err());
    /// ```
    pub fn first_or_fail_fn(&self, callback: impl FnMut(&T) -> bool) -> crate::Result<&T> {
        self.first_where_fn(callback)
            .ok_or_else(|| ItemNotFoundException.into())
    }

    /// Get the last item passing the given truth test.
    pub fn last_where_fn(&self, mut callback: impl FnMut(&T) -> bool) -> Option<&T> {
        self.items.iter().rev().find(|item| callback(item))
    }

    /// Remove the item at the given index.
    pub fn forget(&mut self, index: usize) -> &mut Self {
        if index < self.items.len() {
            self.items.remove(index);
        }
        self
    }

    /// Determine if an item exists at the given index.
    pub fn has(&self, index: usize) -> bool {
        index < self.items.len()
    }

    /// Determine if an item exists at any of the given indexes.
    pub fn has_any(&self, indexes: &[usize]) -> bool {
        indexes.iter().any(|i| *i < self.items.len())
    }

    /// Concatenate the values returned by the callback using the given glue.
    pub fn implode_by(&self, callback: impl FnMut(&T) -> String, glue: &str) -> String {
        self.items.iter().map(callback).collect::<Vec<_>>().join(glue)
    }

    /// Get the indexes ("keys") of the collection.
    pub fn keys(&self) -> Collection<usize> {
        Collection {
            items: (0..self.items.len()).collect(),
        }
    }

    /// Reset the keys of the collection (a no-op for lists, kept for parity).
    pub fn values(self) -> Self {
        self
    }

    /// Get a lazy collection over the items.
    pub fn lazy<'a>(self) -> LazyCollection<'a, T>
    where
        T: 'a,
    {
        LazyCollection::make(self.items)
    }

    /// Map the items into a new type using its `From` implementation.
    pub fn map_into<U: From<T>>(self) -> Collection<U> {
        self.map(U::from)
    }

    /// Group the values returned by the callback by their keys, as plain
    /// lists.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let dictionary = collect(vec![("Sales", "John Doe"), ("Sales", "Jane Doe"), ("Marketing", "Johnny Doe")])
    ///     .map_to_dictionary(|(department, name)| (department, name));
    /// assert_eq!(dictionary["Sales"], vec!["John Doe", "Jane Doe"]);
    /// ```
    pub fn map_to_dictionary<K: Hash + Eq, V>(self, mut callback: impl FnMut(T) -> (K, V)) -> IndexMap<K, Vec<V>> {
        let mut dictionary: IndexMap<K, Vec<V>> = IndexMap::new();
        for item in self.items {
            let (key, value) = callback(item);
            dictionary.entry(key).or_default().push(value);
        }
        dictionary
    }

    /// Add the given items to the beginning of the collection, in order.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// assert_eq!(collect(vec![3, 4]).unshift([1, 2]).all(), &[1, 2, 3, 4]);
    /// ```
    pub fn unshift(mut self, items: impl IntoIterator<Item = T>) -> Self {
        let mut items: Vec<T> = items.into_iter().collect();
        items.append(&mut self.items);
        self.items = items;
        self
    }

    /// Group the values returned by the callback by their keys.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let grouped = collect(vec![("Sales", "John Doe"), ("Sales", "Jane Doe"), ("Marketing", "Johnny Doe")])
    ///     .map_to_groups(|(department, name)| (department, name));
    /// assert_eq!(grouped["Sales"].all(), &["John Doe", "Jane Doe"]);
    /// assert_eq!(grouped["Marketing"].all(), &["Johnny Doe"]);
    /// ```
    pub fn map_to_groups<K: Hash + Eq, V>(
        self,
        mut callback: impl FnMut(T) -> (K, V),
    ) -> IndexMap<K, Collection<V>> {
        let mut groups: IndexMap<K, Collection<V>> = IndexMap::new();
        for item in self.items {
            let (key, value) = callback(item);
            groups.entry(key).or_default().items.push(value);
        }
        groups
    }

    /// Map the items into key / value pairs (later keys win).
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let keyed = collect(vec![("john@example.com", "John"), ("jane@example.com", "Jane")])
    ///     .map_with_keys(|(email, name)| (email.to_string(), name));
    /// assert_eq!(keyed["jane@example.com"], "Jane");
    /// ```
    pub fn map_with_keys<K: Hash + Eq, V>(self, mut callback: impl FnMut(T) -> (K, V)) -> IndexMap<K, V> {
        let mut map = IndexMap::new();
        for item in self.items {
            let (key, value) = callback(item);
            map.insert(key, value);
        }
        map
    }

    /// The percentage of items passing the truth test (two decimals).
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let collection = collect(vec![1, 1, 2, 2, 2, 3]);
    /// assert_eq!(collection.percentage(|n| *n == 1), Some(33.33));
    /// assert_eq!(collection.percentage_with_precision(|n| *n == 1, 3), Some(33.333));
    /// ```
    pub fn percentage(&self, callback: impl FnMut(&T) -> bool) -> Option<f64> {
        self.percentage_with_precision(callback, 2)
    }

    /// The percentage of items passing the truth test, rounded to the given precision.
    pub fn percentage_with_precision(
        &self,
        mut callback: impl FnMut(&T) -> bool,
        precision: i32,
    ) -> Option<f64> {
        if self.items.is_empty() {
            return None;
        }
        let passing = self.items.iter().filter(|item| callback(item)).count();
        let percentage = passing as f64 * 100.0 / self.items.len() as f64;
        let factor = 10f64.powi(precision);
        Some((percentage * factor).round() / factor)
    }

    /// Pass the collection into a new type using its `From` implementation.
    pub fn pipe_into<U: From<Self>>(self) -> U {
        U::from(self)
    }

    /// Remove and return the last `count` items (last item first).
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let mut collection = collect(vec![1, 2, 3, 4, 5]);
    /// assert_eq!(collection.pop_many(3).all(), &[5, 4, 3]);
    /// assert_eq!(collection.all(), &[1, 2]);
    /// ```
    pub fn pop_many(&mut self, count: usize) -> Collection<T> {
        let start = self.items.len().saturating_sub(count);
        let mut popped: Vec<T> = self.items.drain(start..).collect();
        popped.reverse();
        Collection { items: popped }
    }

    /// Remove and return the first `count` items.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let mut collection = collect(vec![1, 2, 3, 4, 5]);
    /// assert_eq!(collection.shift_many(3).all(), &[1, 2, 3]);
    /// assert_eq!(collection.all(), &[4, 5]);
    /// ```
    pub fn shift_many(&mut self, count: usize) -> Collection<T> {
        let end = count.min(self.items.len());
        Collection {
            items: self.items.drain(..end).collect(),
        }
    }

    /// Remove and return the item at the given index.
    pub fn pull(&mut self, index: usize) -> Option<T> {
        (index < self.items.len()).then(|| self.items.remove(index))
    }

    /// Put an item at the given index (pushing it when the index is past the end).
    pub fn put(&mut self, index: usize, item: T) -> &mut Self {
        match self.items.get_mut(index) {
            Some(slot) => *slot = item,
            None => self.items.push(item),
        }
        self
    }

    /// Get a random item from the collection.
    pub fn random_one(&self) -> Option<&T> {
        use rand::seq::IndexedRandom;
        self.items.choose(&mut rand::rng())
    }

    /// Reduce the collection into a mutable accumulator.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let even = collect(vec![1, 2, 3, 4, 5]).reduce_into(Vec::new(), |result, n| {
    ///     if n % 2 == 0 { result.push(n); }
    /// });
    /// assert_eq!(even, vec![2, 4]);
    /// ```
    pub fn reduce_into<A>(self, mut initial: A, mut callback: impl FnMut(&mut A, T)) -> A {
        for item in self.items {
            callback(&mut initial, item);
        }
        initial
    }

    /// Replace the items at the given indexes (appending past the end).
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let replaced = collect(vec!["Taylor", "Abigail", "James"]).replace(vec![(1, "Victoria"), (3, "Finn")]);
    /// assert_eq!(replaced.all(), &["Taylor", "Victoria", "James", "Finn"]);
    /// ```
    pub fn replace(mut self, replacements: impl IntoIterator<Item = (usize, T)>) -> Self {
        for (index, item) in replacements {
            self.put(index, item);
        }
        self
    }

    /// Get the one and only item in the collection.
    pub fn sole(self) -> crate::Result<T> {
        self.sole_fn(|_| true)
    }

    /// Remove a portion of the collection, returning it and inserting the
    /// replacement items in its place.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let mut collection = collect(vec![1, 2, 3, 4, 5]);
    /// let chunk = collection.splice(2, Some(1), vec![10, 11]);
    /// assert_eq!(chunk.all(), &[3]);
    /// assert_eq!(collection.all(), &[1, 2, 10, 11, 4, 5]);
    /// ```
    pub fn splice(
        &mut self,
        offset: usize,
        length: Option<usize>,
        replacement: impl IntoIterator<Item = T>,
    ) -> Collection<T> {
        let start = offset.min(self.items.len());
        let end = match length {
            Some(length) => (start + length).min(self.items.len()),
            None => self.items.len(),
        };
        Collection {
            items: self.items.splice(start..end, replacement).collect(),
        }
    }

    /// Split the collection into the given number of groups, filling
    /// non-terminal groups completely.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let groups = collect(1..=10).split_in(3);
    /// assert_eq!(groups.map(|g| g.into_vec()).into_vec(), vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8], vec![9, 10]]);
    /// ```
    pub fn split_in(self, groups: usize) -> Collection<Collection<T>> {
        let groups = groups.max(1);
        let size = self.items.len().div_ceil(groups);
        self.chunk(size)
    }

    /// Apply the callback if the collection is empty.
    pub fn when_empty(self, callback: impl FnOnce(Self) -> Self) -> Self {
        if self.items.is_empty() { callback(self) } else { self }
    }

    /// Apply the callback if the collection is not empty.
    pub fn when_not_empty(self, callback: impl FnOnce(Self) -> Self) -> Self {
        if self.items.is_empty() { self } else { callback(self) }
    }

    /// Apply the callback unless the collection is empty.
    pub fn unless_empty(self, callback: impl FnOnce(Self) -> Self) -> Self {
        self.when_not_empty(callback)
    }

    /// Apply the callback unless the collection is not empty.
    pub fn unless_not_empty(self, callback: impl FnOnce(Self) -> Self) -> Self {
        self.when_empty(callback)
    }

    /// Get the underlying items from the collection.
    pub fn unwrap(self) -> Vec<T> {
        self.items
    }

    /// Create a collection from a JSON array.
    pub fn from_json(json: &str) -> crate::Result<Self>
    where
        T: DeserializeOwned,
    {
        Ok(serde_json::from_str(json)?)
    }
}

impl<T: Clone> Collection<T> {
    /// Cross join the collection with the given items.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let matrix = collect(vec![1, 2]).cross_join(vec!["a", "b"]);
    /// assert_eq!(matrix.into_vec(), vec![(1, "a"), (1, "b"), (2, "a"), (2, "b")]);
    /// ```
    pub fn cross_join<U: Clone>(self, other: impl IntoIterator<Item = U>) -> Collection<(T, U)> {
        let other: Vec<U> = other.into_iter().collect();
        Collection {
            items: self
                .items
                .into_iter()
                .flat_map(|a| other.iter().map(move |b| (a.clone(), b.clone())))
                .collect(),
        }
    }

    /// Cross join the collection with several lists of the same type.
    pub fn cross_join_many<I>(self, others: impl IntoIterator<Item = I>) -> Collection<Vec<T>>
    where
        I: IntoIterator<Item = T>,
    {
        let mut results: Vec<Vec<T>> = self.items.into_iter().map(|item| vec![item]).collect();
        for other in others {
            let other: Vec<T> = other.into_iter().collect();
            let mut next = Vec::with_capacity(results.len() * other.len());
            for product in &results {
                for item in &other {
                    let mut combined = product.clone();
                    combined.push(item.clone());
                    next.push(combined);
                }
            }
            results = next;
        }
        Collection { items: results }
    }

    /// Repeat the collection's items the given number of times.
    pub fn multiply(self, times: usize) -> Self {
        let mut items = Vec::with_capacity(self.items.len() * times);
        for _ in 0..times {
            items.extend(self.items.iter().cloned());
        }
        Self { items }
    }

    /// Get the item at the given index, or the default.
    pub fn get_or(&self, index: usize, default: T) -> T {
        self.items.get(index).cloned().unwrap_or(default)
    }

    /// Get the keys (as returned by the callback) that appear more than once.
    pub fn duplicates_by<K: Hash + Eq + Clone>(&self, mut key: impl FnMut(&T) -> K) -> Collection<K> {
        let mut seen = HashSet::new();
        let mut dupes: Vec<K> = Vec::new();
        for item in &self.items {
            let k = key(item);
            if !seen.insert(k.clone()) && !dupes.contains(&k) {
                dupes.push(k);
            }
        }
        Collection { items: dupes }
    }
}

impl<T: PartialEq> Collection<T> {
    /// Get the item after the given item.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let collection = collect(vec![1, 2, 3, 4, 5]);
    /// assert_eq!(collection.after(&3), Some(&4));
    /// assert_eq!(collection.after(&5), None);
    /// ```
    pub fn after(&self, item: &T) -> Option<&T> {
        self.after_fn(|i| i == item)
    }

    /// Get the item before the given item.
    pub fn before(&self, item: &T) -> Option<&T> {
        self.before_fn(|i| i == item)
    }
}

impl<T: Hash + Eq + Clone> Collection<T> {
    /// Get the most frequent items.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// assert_eq!(collect(vec![1, 1, 2, 4]).mode().all(), &[1]);
    /// assert_eq!(collect(vec![1, 1, 2, 2]).mode().all(), &[1, 2]);
    /// ```
    pub fn mode(&self) -> Collection<T> {
        let counts = self.count_by_value();
        let Some(highest) = counts.values().max().copied() else {
            return Collection::new();
        };
        counts
            .into_iter()
            .filter(|(_, count)| *count == highest)
            .map(|(item, _)| item)
            .collect()
    }

    /// Count the occurrences of each item.
    pub fn count_by_value(&self) -> IndexMap<T, usize> {
        self.count_by(|item| item.clone())
    }

    /// Combine the items (as keys) with the given values.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let combined = collect(vec!["name", "age"]).combine(vec!["George", "29"]);
    /// assert_eq!(combined["name"], "George");
    /// assert_eq!(combined["age"], "29");
    /// ```
    pub fn combine<V>(self, values: impl IntoIterator<Item = V>) -> IndexMap<T, V> {
        self.items.into_iter().zip(values).collect()
    }
}

/// Methods for collections of pairs.
impl<A, B> Collection<(A, B)> {
    /// Run a map over each pair, spreading it into the callback's arguments.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let sums = collect(vec![(0, 1), (2, 3), (4, 5)]).map_spread(|even, odd| even + odd);
    /// assert_eq!(sums.all(), &[1, 5, 9]);
    /// ```
    pub fn map_spread<U>(self, mut callback: impl FnMut(A, B) -> U) -> Collection<U> {
        self.map(|(a, b)| callback(a, b))
    }

    /// Execute a callback over each pair, spreading it into the arguments.
    pub fn each_spread(&self, mut callback: impl FnMut(&A, &B)) -> &Self {
        for (a, b) in &self.items {
            callback(a, b);
        }
        self
    }

    /// Split the pairs into two collections.
    pub fn unzip(self) -> (Collection<A>, Collection<B>) {
        let (a, b): (Vec<A>, Vec<B>) = self.items.into_iter().unzip();
        (Collection { items: a }, Collection { items: b })
    }

    /// Collect the pairs into an insertion-ordered map.
    pub fn into_map(self) -> IndexMap<A, B>
    where
        A: Hash + Eq,
    {
        self.items.into_iter().collect()
    }
}

/// Methods for keyed collections: pairs of keys and values, the way PHP's
/// associative arrays work.
impl<K: Hash + Eq, V> Collection<(K, V)> {
    /// Get the pairs whose keys aren't in the given pairs.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let diff = collect(vec![("one", 10), ("two", 20), ("three", 30)]).diff_keys(vec![("two", 2), ("four", 4)]);
    /// assert_eq!(diff.all(), &[("one", 10), ("three", 30)]);
    /// ```
    pub fn diff_keys<W>(self, other: impl IntoIterator<Item = (K, W)>) -> Self {
        let keys: HashSet<K> = other.into_iter().map(|(key, _)| key).collect();
        Collection {
            items: self.items.into_iter().filter(|(key, _)| !keys.contains(key)).collect(),
        }
    }

    /// Get the pairs whose keys are in the given pairs.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let intersect = collect(vec![("serial", "UX301"), ("type", "screen"), ("year", "2009")])
    ///     .intersect_by_keys(vec![("reference", "UX404"), ("type", "tab"), ("year", "2011")]);
    /// assert_eq!(intersect.all(), &[("type", "screen"), ("year", "2009")]);
    /// ```
    pub fn intersect_by_keys<W>(self, other: impl IntoIterator<Item = (K, W)>) -> Self {
        let keys: HashSet<K> = other.into_iter().map(|(key, _)| key).collect();
        Collection {
            items: self.items.into_iter().filter(|(key, _)| keys.contains(key)).collect(),
        }
    }

    /// Sort the collection by its keys.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let sorted = collect(vec![("id", 22345), ("first", "John".len()), ("last", "Doe".len())]).sort_keys();
    /// assert_eq!(sorted.keys_of(), vec!["first", "id", "last"]);
    /// ```
    pub fn sort_keys(self) -> Self
    where
        K: Ord,
    {
        self.sort_keys_using(|a, b| a.cmp(b))
    }

    /// Sort the collection by its keys, in descending order.
    pub fn sort_keys_desc(self) -> Self
    where
        K: Ord,
    {
        self.sort_keys_using(|a, b| b.cmp(a))
    }

    /// Sort the collection by its keys with the given comparison.
    pub fn sort_keys_using(mut self, mut compare: impl FnMut(&K, &K) -> Ordering) -> Self {
        self.items.sort_by(|(a, _), (b, _)| compare(a, b));
        self
    }

    /// The keys, in order (`keys` returns the positions, like a list).
    pub fn keys_of(&self) -> Vec<K>
    where
        K: Clone,
    {
        self.items.iter().map(|(key, _)| key.clone()).collect()
    }

    /// Get the value for the key, or add the given default and return it.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let mut collection = collect(vec![("name", "Taylor".to_string())]);
    /// assert_eq!(collection.get_or_put("name", || "Abigail".into()), "Taylor");
    /// assert_eq!(collection.get_or_put("role", || "Developer".into()), "Developer");
    /// assert_eq!(collection.count(), 2);
    /// ```
    pub fn get_or_put(&mut self, key: K, value: impl FnOnce() -> V) -> &mut V {
        let index = match self.items.iter().position(|(existing, _)| *existing == key) {
            Some(index) => index,
            None => {
                self.items.push((key, value()));
                self.items.len() - 1
            }
        };
        &mut self.items[index].1
    }

    /// Add the given pairs whose keys aren't in the collection yet.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let union = collect(vec![(1, "a"), (2, "b")]).union(vec![(3, "c"), (1, "d")]);
    /// assert_eq!(union.all(), &[(1, "a"), (2, "b"), (3, "c")]);
    /// ```
    pub fn union(mut self, other: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Clone,
    {
        let mut keys: HashSet<K> = self.items.iter().map(|(key, _)| key.clone()).collect();
        for (key, value) in other {
            if keys.insert(key.clone()) {
                self.items.push((key, value));
            }
        }
        self
    }

    /// Swap the keys and values. When values repeat, the last key wins.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let flipped = collect(vec![("name", "taylor"), ("framework", "laravel")]).flip();
    /// assert_eq!(flipped.all(), &[("taylor", "name"), ("laravel", "framework")]);
    /// ```
    pub fn flip(self) -> Collection<(V, K)>
    where
        V: Hash + Eq,
    {
        let flipped: IndexMap<V, K> = self.items.into_iter().map(|(key, value)| (value, key)).collect();
        flipped.into_iter().collect()
    }

    /// Get the pairs that aren't in the given pairs, comparing keys and
    /// values.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let diff = collect(vec![("color", "orange"), ("type", "fruit"), ("remain", "6")])
    ///     .diff_assoc(vec![("color", "yellow"), ("type", "fruit"), ("remain", "3"), ("used", "6")]);
    /// assert_eq!(diff.all(), &[("color", "orange"), ("remain", "6")]);
    /// ```
    pub fn diff_assoc(self, other: impl IntoIterator<Item = (K, V)>) -> Self
    where
        V: PartialEq,
    {
        let other: Vec<(K, V)> = other.into_iter().collect();
        Collection {
            items: self.items.into_iter().filter(|pair| !other.contains(pair)).collect(),
        }
    }

    /// Get the pairs that are also in the given pairs, comparing keys and
    /// values.
    ///
    /// ```
    /// use illuminate_support::collect;
    ///
    /// let intersect = collect(vec![("color", "red"), ("size", "M"), ("material", "cotton")])
    ///     .intersect_assoc(vec![("color", "blue"), ("size", "M"), ("material", "polyester")]);
    /// assert_eq!(intersect.all(), &[("size", "M")]);
    /// ```
    pub fn intersect_assoc(self, other: impl IntoIterator<Item = (K, V)>) -> Self
    where
        V: PartialEq,
    {
        let other: Vec<(K, V)> = other.into_iter().collect();
        Collection {
            items: self.items.into_iter().filter(|pair| other.contains(pair)).collect(),
        }
    }
}

/// Methods for collections of triples.
impl<A, B, C> Collection<(A, B, C)> {
    /// Run a map over each triple, spreading it into the callback's arguments.
    pub fn map_spread<U>(self, mut callback: impl FnMut(A, B, C) -> U) -> Collection<U> {
        self.map(|(a, b, c)| callback(a, b, c))
    }

    /// Execute a callback over each triple, spreading it into the arguments.
    pub fn each_spread(&self, mut callback: impl FnMut(&A, &B, &C)) -> &Self {
        for (a, b, c) in &self.items {
            callback(a, b, c);
        }
        self
    }
}

impl Collection<i64> {
    /// Create a collection with the given range of integers (descending when
    /// `from` is greater than `to`).
    ///
    /// ```
    /// use illuminate_support::Collection;
    ///
    /// assert_eq!(Collection::range(3, 6).all(), &[3, 4, 5, 6]);
    /// assert_eq!(Collection::range(3, 1).all(), &[3, 2, 1]);
    /// ```
    pub fn range(from: i64, to: i64) -> Self {
        if from <= to {
            Self::make(from..=to)
        } else {
            Self::make((to..=from).rev())
        }
    }

    /// Create a collection with the given range of integers, stepping by `step`.
    pub fn range_step(from: i64, to: i64, step: i64) -> Self {
        let step = step.unsigned_abs().max(1) as usize;
        if from <= to {
            Self::make((from..=to).step_by(step))
        } else {
            Self::make((to..=from).rev().step_by(step))
        }
    }
}

/// Methods for the dynamic "PHP array" collection.
impl Collection<Value> {
    /// Wrap the given value in a collection: lists become items, `null`
    /// becomes empty, and anything else becomes a single item.
    ///
    /// ```
    /// use illuminate_support::{Collection, json};
    ///
    /// assert_eq!(Collection::wrap("John Doe").all(), &[json!("John Doe")]);
    /// assert_eq!(Collection::wrap(json!(["John Doe"])).all(), &[json!("John Doe")]);
    /// ```
    pub fn wrap(value: impl Into<Value>) -> Self {
        match value.into() {
            Value::Null => Self::new(),
            Value::Array(items) => Self { items },
            other => Self { items: vec![other] },
        }
    }

    /// Create a collection from a value: lists and objects contribute their
    /// values, `null` is empty, and scalars become a single item.
    pub fn from_value(value: Value) -> Self {
        match value {
            Value::Null => Self::new(),
            Value::Array(items) => Self { items },
            Value::Object(map) => Self {
                items: map.into_iter().map(|(_, v)| v).collect(),
            },
            other => Self { items: vec![other] },
        }
    }

    /// Collapse a collection of arrays into a single, flat collection.
    pub fn collapse(self) -> Self {
        let mut items = Vec::new();
        for item in self.items {
            match item {
                Value::Array(values) => items.extend(values),
                Value::Object(map) => items.extend(map.into_iter().map(|(_, v)| v)),
                _ => {}
            }
        }
        Self { items }
    }

    /// Flatten nested arrays into a single level (to the given depth).
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let flattened = collect(vec![json!("Taylor"), json!(["PHP", "JavaScript"])]).flatten(None);
    /// assert_eq!(flattened.all(), &[json!("Taylor"), json!("PHP"), json!("JavaScript")]);
    /// ```
    pub fn flatten(self, depth: Option<usize>) -> Self {
        let wrapper = Value::Array(self.items);
        Self {
            items: crate::arr::Arr::flatten(&wrapper, depth),
        }
    }

    /// Remove every "falsy" value (null, false, 0, "", "0" and empty arrays),
    /// like calling Laravel's `filter()` without a callback.
    pub fn filter_truthy(self) -> Self {
        self.filter(|v| v.truthy())
    }
}

/// Serialization-powered methods ("dot" notation keys) for any serializable item.
impl<T: Serialize> Collection<T> {
    /// Determine if any item's key loosely equals the given value.
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let products = collect(vec![json!({"product": "Desk", "price": 200}), json!({"product": "Chair", "price": 100})]);
    /// assert!(!products.contains_where("product", "Bookcase"));
    /// assert!(products.doesnt_contain_where("product", "Bookcase"));
    /// ```
    pub fn contains_where(&self, key: &str, value: impl Into<Value>) -> bool {
        let value = value.into();
        self.items
            .iter()
            .any(|item| loose_eq(&to_value(item).dot_or_null(key), &value))
    }

    /// Determine if no item's key loosely equals the given value.
    pub fn doesnt_contain_where(&self, key: &str, value: impl Into<Value>) -> bool {
        !self.contains_where(key, value)
    }

    /// Determine if any item loosely equals the given value.
    pub fn contains_value(&self, value: impl Into<Value>) -> bool {
        let value = value.into();
        self.items.iter().any(|item| loose_eq(&to_value(item), &value))
    }

    /// Get the values of the given key that appear more than once.
    pub fn duplicates_key(&self, key: &str) -> Collection<Value> {
        let mut seen: Vec<Value> = Vec::new();
        let mut dupes: Vec<Value> = Vec::new();
        for item in &self.items {
            let value = to_value(item).dot_or_null(key);
            if seen.iter().any(|s| loose_eq(s, &value)) {
                if !dupes.iter().any(|d| loose_eq(d, &value)) {
                    dupes.push(value);
                }
            } else {
                seen.push(value);
            }
        }
        Collection { items: dupes }
    }

    /// Count the items by the (string) value of the given key.
    pub fn count_by_key(&self, key: &str) -> IndexMap<String, usize> {
        self.count_by(|item| to_value(item).dot_or_null(key).to_string_lossy())
    }

    /// Concatenate the values of the given key using the glue.
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let products = collect(vec![json!({"product": "Desk"}), json!({"product": "Chair"})]);
    /// assert_eq!(products.implode_key("product", ", "), "Desk, Chair");
    /// ```
    pub fn implode_key(&self, key: &str, glue: &str) -> String {
        self.implode_by(|item| to_value(item).dot_or_null(key).to_string_lossy(), glue)
    }

    /// Get the values of the given key, keyed by the value of another key.
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let cars = collect(vec![
    ///     json!({"brand": "Tesla", "color": "red"}),
    ///     json!({"brand": "Pagani", "color": "white"}),
    ///     json!({"brand": "Tesla", "color": "black"}),
    /// ]);
    /// let plucked = cars.pluck_with_key("color", "brand");
    /// assert_eq!(plucked["Tesla"], json!("black"));
    /// assert_eq!(plucked["Pagani"], json!("white"));
    /// ```
    pub fn pluck_with_key(&self, value: &str, key: &str) -> IndexMap<String, Value> {
        let mut map = IndexMap::new();
        for item in &self.items {
            let item = to_value(item);
            map.insert(item.dot_or_null(key).to_string_lossy(), item.dot_or_null(value));
        }
        map
    }

    /// Select only the given (top-level) keys from each item.
    pub fn select(&self, keys: &[&str]) -> Collection<Value> {
        Collection {
            items: self
                .items
                .iter()
                .map(|item| {
                    let item = to_value(item);
                    let mut out = Map::new();
                    for key in keys {
                        if let Some(value) = item.get(*key) {
                            out.insert((*key).to_string(), value.clone());
                        }
                    }
                    Value::Object(out)
                })
                .collect(),
        }
    }

    /// Get the median of the numeric values of the given key.
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let items = collect(vec![json!({"foo": 10}), json!({"foo": 10}), json!({"foo": 20}), json!({"foo": 40})]);
    /// assert_eq!(items.median_key("foo"), Some(15.0));
    /// assert_eq!(items.mode_key("foo").all(), &[json!(10)]);
    /// ```
    pub fn median_key(&self, key: &str) -> Option<f64> {
        let mut values: Vec<f64> = self
            .items
            .iter()
            .filter_map(|item| to_value(item).dot_or_null(key).to_f64_lossy())
            .collect();
        if values.is_empty() {
            return None;
        }
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        let mid = values.len() / 2;
        Some(if values.len().is_multiple_of(2) {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        })
    }

    /// Get the most frequent values of the given key.
    pub fn mode_key(&self, key: &str) -> Collection<Value> {
        let mut counts: IndexMap<String, (Value, usize)> = IndexMap::new();
        for item in &self.items {
            let value = to_value(item).dot_or_null(key);
            if value.is_null() {
                continue;
            }
            counts.entry(value.to_string()).or_insert((value, 0)).1 += 1;
        }
        let Some(highest) = counts.values().map(|(_, c)| *c).max() else {
            return Collection::new();
        };
        counts
            .into_values()
            .filter(|(_, count)| *count == highest)
            .map(|(value, _)| value)
            .collect()
    }

    /// Return only the items with a unique value for the given key.
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let phones = collect(vec![
    ///     json!({"name": "iPhone 6", "brand": "Apple"}),
    ///     json!({"name": "iPhone 5", "brand": "Apple"}),
    ///     json!({"name": "Galaxy S6", "brand": "Samsung"}),
    /// ]);
    /// assert_eq!(phones.unique_by_key("brand").pluck("name").all(), &[json!("iPhone 6"), json!("Galaxy S6")]);
    /// ```
    pub fn unique_by_key(self, key: &str) -> Self {
        self.unique_by(|item| to_value(item).dot_or_null(key).to_string_lossy())
    }

    /// Get the first item where the given key passes the comparison.
    pub fn first_where_op(&self, key: &str, operator: &str, value: impl Into<Value>) -> Option<&T> {
        let value = value.into();
        self.items
            .iter()
            .find(|item| compare_values(&to_value(*item).dot_or_null(key), operator, &value))
    }

    /// Get the one and only item where the given key loosely equals the value.
    pub fn sole_where(self, key: &str, value: impl Into<Value>) -> crate::Result<T> {
        let value = value.into();
        self.sole_fn(|item| loose_eq(&to_value(item).dot_or_null(key), &value))
    }

    /// Filter items where the given key strictly equals the given value.
    pub fn where_strict(self, key: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.filter(|item| to_value(item).dot_or_null(key) == value)
    }

    /// Filter items where the given key is strictly one of the given values.
    pub fn where_in_strict(self, key: &str, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        self.filter(|item| values.contains(&to_value(item).dot_or_null(key)))
    }

    /// Filter items where the given key is strictly not one of the given values.
    pub fn where_not_in_strict(
        self,
        key: &str,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        self.filter(|item| !values.contains(&to_value(item).dot_or_null(key)))
    }

    /// Filter items where the given key is between the two values (inclusive).
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let products = collect(vec![json!({"price": 200}), json!({"price": 80}), json!({"price": 150})]);
    /// assert_eq!(products.clone().where_between("price", [100, 200]).count(), 2);
    /// assert_eq!(products.where_not_between("price", [100, 200]).count(), 1);
    /// ```
    pub fn where_between<V: Into<Value>>(self, key: &str, range: [V; 2]) -> Self {
        let [from, to] = range.map(Into::into);
        self.filter(|item| {
            let value = to_value(item).dot_or_null(key);
            compare_for_sort(&value, &from) != Ordering::Less
                && compare_for_sort(&value, &to) != Ordering::Greater
        })
    }

    /// Filter items where the given key is outside the two values.
    pub fn where_not_between<V: Into<Value>>(self, key: &str, range: [V; 2]) -> Self {
        let [from, to] = range.map(Into::into);
        self.filter(|item| {
            let value = to_value(item).dot_or_null(key);
            compare_for_sort(&value, &from) == Ordering::Less
                || compare_for_sort(&value, &to) == Ordering::Greater
        })
    }

    /// Sort the collection by several keys, each `"asc"` or `"desc"`.
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let people = collect(vec![
    ///     json!({"name": "Taylor Otwell", "age": 34}),
    ///     json!({"name": "Abigail Otwell", "age": 30}),
    ///     json!({"name": "Taylor Otwell", "age": 36}),
    ///     json!({"name": "Abigail Otwell", "age": 32}),
    /// ]);
    /// let sorted = people.sort_by_many(&[("name", "asc"), ("age", "desc")]);
    /// assert_eq!(sorted.pluck("age").all(), &[json!(32), json!(30), json!(36), json!(34)]);
    /// ```
    pub fn sort_by_many(self, criteria: &[(&str, &str)]) -> Self {
        let mut keyed: Vec<(Vec<Value>, T)> = self
            .items
            .into_iter()
            .map(|item| {
                let value = to_value(&item);
                let keys = criteria.iter().map(|(key, _)| value.dot_or_null(key)).collect();
                (keys, item)
            })
            .collect();
        keyed.sort_by(|(a, _), (b, _)| {
            for (index, (_, direction)) in criteria.iter().enumerate() {
                let ordering = compare_for_sort(&a[index], &b[index]);
                let ordering = if direction.eq_ignore_ascii_case("desc") {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            Ordering::Equal
        });
        Self {
            items: keyed.into_iter().map(|(_, item)| item).collect(),
        }
    }

    /// Get the value of the given key from the first item where it is truthy.
    ///
    /// ```
    /// use illuminate_support::{collect, json};
    ///
    /// let products = collect(vec![json!({"product": "Desk", "price": 200}), json!({"product": "Speaker", "price": 400})]);
    /// assert_eq!(products.value("price"), json!(200));
    /// ```
    pub fn value(&self, key: &str) -> Value {
        self.items
            .iter()
            .map(|item| to_value(item).dot_or_null(key))
            .find(|value| value.truthy())
            .unwrap_or(Value::Null)
    }
}

impl<K, V> From<IndexMap<K, V>> for Collection<(K, V)> {
    fn from(map: IndexMap<K, V>) -> Self {
        Collection {
            items: map.into_iter().collect(),
        }
    }
}

impl<K, V> From<HashMap<K, V>> for Collection<(K, V)> {
    fn from(map: HashMap<K, V>) -> Self {
        Collection {
            items: map.into_iter().collect(),
        }
    }
}

impl From<Value> for Collection<Value> {
    fn from(value: Value) -> Self {
        Collection::from_value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect;
    use serde_json::json;

    #[test]
    fn it_finds_items_around_others() {
        let collection = collect(vec![1, 2, 3, 4, 5]);
        assert_eq!(collection.after(&3), Some(&4));
        assert_eq!(collection.after(&5), None);
        assert_eq!(collection.before(&3), Some(&2));
        assert_eq!(collection.before(&1), None);
        assert_eq!(collect(vec![2, 4, 6, 8]).before_fn(|n| *n > 5), Some(&4));
    }

    #[test]
    fn it_mutates_like_laravel() {
        let mut collection = collect(vec![1, 2, 3, 4, 5]);
        assert_eq!(collection.splice(2, None, vec![]).all(), &[3, 4, 5]);
        assert_eq!(collection.all(), &[1, 2]);
        let mut collection = collect(vec![1, 2, 3, 4, 5]);
        assert_eq!(collection.splice(2, Some(1), vec![]).all(), &[3]);
        assert_eq!(collection.all(), &[1, 2, 4, 5]);

        let mut collection = collect(vec!["product", "name"]);
        assert_eq!(collection.pull(1), Some("name"));
        assert_eq!(collection.pull(5), None);
        collection.put(0, "id").put(9, "price");
        assert_eq!(collection.all(), &["id", "price"]);
        collection.forget(0);
        assert_eq!(collection.all(), &["price"]);
        assert!(collection.has(0));
        assert!(!collection.has(1));
    }

    #[test]
    fn it_maps_into_groups_and_keys() {
        let people = collect(vec![
            json!({"name": "John Doe", "department": "Sales"}),
            json!({"name": "Jane Doe", "department": "Sales"}),
            json!({"name": "Johnny Doe", "department": "Marketing"}),
        ]);
        let grouped = people.clone().map_to_groups(|p| {
            (p["department"].as_str().unwrap().to_string(), p["name"].clone())
        });
        assert_eq!(grouped["Sales"].all(), &[json!("John Doe"), json!("Jane Doe")]);
        let keyed = people.map_with_keys(|p| (p["name"].as_str().unwrap().to_string(), p["department"].clone()));
        assert_eq!(keyed["Johnny Doe"], json!("Marketing"));
    }

    #[test]
    fn it_uses_serializable_helpers() {
        let employees = collect(vec![
            json!({"email": "abigail@example.com", "position": "Developer"}),
            json!({"email": "james@example.com", "position": "Designer"}),
            json!({"email": "victoria@example.com", "position": "Developer"}),
        ]);
        assert_eq!(employees.duplicates_key("position").all(), &[json!("Developer")]);
        assert_eq!(employees.count_by_key("position")["Developer"], 2);
        assert_eq!(employees.select(&["position"]).first(), Some(&json!({"position": "Developer"})));
        assert!(employees.contains_value(json!({"email": "james@example.com", "position": "Designer"})));

        let ages = collect(vec![
            json!({"name": "Regena", "age": null}),
            json!({"name": "Linda", "age": 14}),
            json!({"name": "Diego", "age": 23}),
            json!({"name": "Linda", "age": 84}),
        ]);
        assert_eq!(ages.first_where("name", "Linda").unwrap()["age"], json!(14));
        assert_eq!(ages.first_where_op("age", ">=", 18).unwrap()["name"], json!("Diego"));
        assert_eq!(ages.value("age"), json!(14));
        assert!(ages.clone().sole_where("name", "Diego").is_ok());
        assert!(ages.clone().sole_where("name", "Linda").is_err());
        assert_eq!(ages.clone().where_strict("age", 14).count(), 1);
        assert_eq!(ages.clone().where_in_strict("age", [14, 23]).count(), 2);
        assert_eq!(ages.where_not_in_strict("age", [14, 23]).count(), 2);

        let numbers = collect(vec![json!(1), json!(1), json!(2), json!(4)]);
        assert_eq!(numbers.median_key(""), Some(1.5));
        assert_eq!(numbers.mode_key("").all(), &[json!(1)]);
    }

    #[test]
    fn it_handles_value_collections() {
        let values = collect(vec![json!(1), json!(2), json!(3), json!(null), json!(false), json!(""), json!(0), json!([])]);
        assert_eq!(values.filter_truthy().all(), &[json!(1), json!(2), json!(3)]);
        let nested = collect(vec![json!([1, 2, 3]), json!([4, 5, 6]), json!([7, 8, 9])]);
        assert_eq!(nested.collapse().count(), 9);
        let deep = collect(vec![json!({"Apple": [{"name": "iPhone 6S"}]}), json!({"Samsung": [{"name": "Galaxy S7"}]})]);
        assert_eq!(deep.flatten(Some(1)).all(), &[json!([{"name": "iPhone 6S"}]), json!([{"name": "Galaxy S7"}])]);
        assert!(Collection::wrap(Value::Null).is_empty());
        assert_eq!(Collection::from_value(json!({"a": 1, "b": 2})).all(), &[json!(1), json!(2)]);
    }

    #[test]
    fn it_supports_misc_methods() {
        assert_eq!(collect(vec![1, 2, 3]).sole().unwrap_err().to_string(), "3 items were found.");
        assert_eq!(collect(vec![1]).sole().unwrap(), 1);
        assert!(collect(Vec::<i32>::new()).first_or_fail().is_err());
        assert!(collect(vec![1]).contains_one_item());
        assert!(collect(vec![1, 2]).has_many());
        assert!(!collect(vec![json!({"age": 2}), json!({"age": 3})]).has_many_fn(|i| i["age"] == 2));
        assert!(collect(vec![1, 2, 3]).has_sole_fn(|n| *n == 2));
        assert_eq!(collect(vec!["a", "b", "c", "d"]).except(&[1, 3]).all(), &["a", "c"]);
        assert_eq!(collect(vec!["a", "b", "c", "d"]).only(&[1, 3]).all(), &["b", "d"]);
        assert_eq!(collect(vec![1, 2]).multiply(3).all(), &[1, 2, 1, 2, 1, 2]);
        assert_eq!(collect(vec![1, 2, 3]).keys().all(), &[0, 1, 2]);
        assert_eq!(
            collect(vec![1, 2]).cross_join_many(vec![vec![3, 4], vec![5, 6]]).count(),
            8
        );
        assert_eq!(collect(vec!["a", "b", "a", "c", "b"]).duplicates_by(|s| *s).all(), &["a", "b"]);
        assert_eq!(collect(vec![1, 2, 3]).when_empty(|c| c.push(4)).all(), &[1, 2, 3]);
        assert_eq!(collect(Vec::<i32>::new()).when_empty(|c| c.push(4)).all(), &[4]);
        assert_eq!(collect(vec![1]).when_not_empty(|c| c.push(4)).all(), &[1, 4]);
        assert_eq!(collect(vec![1]).unless_empty(|c| c.push(4)).all(), &[1, 4]);
        assert_eq!(collect(Vec::<i32>::new()).unless_not_empty(|c| c.push(4)).all(), &[4]);
        assert_eq!(collect(vec![1, 2, 3]).implode_by(|n| (n * 2).to_string(), "-"), "2-4-6");
        assert!(collect(vec![1, 2, 3]).random_one().is_some());
        assert_eq!(Collection::<i64>::from_json("[1,2,3]").unwrap().sum(), 6);
        let (letters, numbers) = collect(vec![("a", 1), ("b", 2)]).unzip();
        assert_eq!((letters.all(), numbers.all()), (&["a", "b"][..], &[1, 2][..]));
        assert_eq!(collect(vec![("a", 1, true)]).map_spread(|a, n, b| format!("{a}{n}{b}")).all(), &["a1true"]);
        assert_eq!(Collection::range_step(0, 10, 5).all(), &[0, 5, 10]);
        let map: IndexMap<&str, i32> = IndexMap::from([("a", 1)]);
        assert_eq!(Collection::from(map).into_map()["a"], 1);
        assert!(collect(vec![3, 1]).doesnt_contain_fn(|n| *n > 5));
        assert!(collect(vec![3, 1]).some(|n| *n > 2));
    }
}
