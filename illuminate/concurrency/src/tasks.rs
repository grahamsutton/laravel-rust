//! The shapes of task lists `Concurrency::run` accepts: tuples, arrays,
//! vectors, and keyed maps of futures.

use std::any::Any;
use std::future::Future;
use std::hash::Hash;

use futures::FutureExt;
use futures::future::BoxFuture;
use illuminate_support::Result;
use indexmap::IndexMap;

/// The type-erased value a job produces.
pub type JobOutput = Box<dyn Any + Send>;

/// A type-erased task, ready to be handed to a [`Driver`](crate::Driver).
pub type Job = BoxFuture<'static, Result<JobOutput>>;

/// Puts a driver's job outputs back together into the caller's shape.
pub type Assembler<T> = Box<dyn FnOnce(Vec<JobOutput>) -> T + Send>;

/// A boxed task: the type to reach for when building a `Vec` of tasks
/// whose futures have different types. See [`task`].
pub type Task<T> = BoxFuture<'static, Result<T>>;

/// Box a future so it can sit in a `Vec` (or map) alongside others.
///
/// ```
/// use illuminate_concurrency::{Concurrency, task};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let reports = vec![
///     task(async { Ok("users") }),
///     task(async { Ok("orders") }),
/// ];
///
/// assert_eq!(Concurrency::run(reports).await?, ["users", "orders"]);
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub fn task<T, F>(future: F) -> Task<T>
where
    F: Future<Output = Result<T>> + Send + 'static,
{
    future.boxed()
}

/// A list of tasks that can run concurrently, and the shape of its results.
///
/// Tasks are futures returning `illuminate_support::Result<T>`:
///
/// - a tuple of futures (of any types) produces a tuple of results,
/// - an array or `Vec` of futures produces an array or `Vec` of results,
/// - an [`IndexMap`] of futures produces an `IndexMap` of results with the
///   same keys (Laravel's "named results").
pub trait Tasks {
    /// The results of the tasks.
    type Output: Send + 'static;

    /// Split the tasks into type-erased jobs, plus a way to assemble their
    /// outputs back into [`Self::Output`].
    fn into_jobs(self) -> (Vec<Job>, Assembler<Self::Output>);
}

fn erase<F, T>(future: F) -> Job
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    future
        .map(|result| result.map(|value| Box::new(value) as JobOutput))
        .boxed()
}

fn restore<T: 'static>(output: JobOutput) -> T {
    *output
        .downcast::<T>()
        .unwrap_or_else(|_| panic!("A concurrent task produced an unexpected type of value."))
}

impl<F, T> Tasks for Vec<F>
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    type Output = Vec<T>;

    fn into_jobs(self) -> (Vec<Job>, Assembler<Vec<T>>) {
        let jobs = self.into_iter().map(erase).collect();
        (
            jobs,
            Box::new(|outputs| outputs.into_iter().map(restore).collect()),
        )
    }
}

impl<F, T, const N: usize> Tasks for [F; N]
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    type Output = [T; N];

    fn into_jobs(self) -> (Vec<Job>, Assembler<[T; N]>) {
        let jobs = self.into_iter().map(erase).collect();
        (
            jobs,
            Box::new(|outputs| {
                let values: Vec<T> = outputs.into_iter().map(restore).collect();
                values
                    .try_into()
                    .unwrap_or_else(|_| panic!("A concurrent task did not report a result."))
            }),
        )
    }
}

impl<K, F, T> Tasks for IndexMap<K, F>
where
    K: Hash + Eq + Send + 'static,
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    type Output = IndexMap<K, T>;

    fn into_jobs(self) -> (Vec<Job>, Assembler<IndexMap<K, T>>) {
        let (keys, jobs): (Vec<K>, Vec<Job>) = self
            .into_iter()
            .map(|(key, future)| (key, erase(future)))
            .unzip();
        (
            jobs,
            Box::new(move |outputs| {
                keys.into_iter()
                    .zip(outputs.into_iter().map(restore))
                    .collect()
            }),
        )
    }
}

macro_rules! tuple_tasks {
    ($(($future:ident, $value:ident, $index:tt)),+) => {
        impl<$($future, $value),+> Tasks for ($($future,)+)
        where
            $(
                $future: Future<Output = Result<$value>> + Send + 'static,
                $value: Send + 'static,
            )+
        {
            type Output = ($($value,)+);

            fn into_jobs(self) -> (Vec<Job>, Assembler<Self::Output>) {
                let jobs = vec![$(erase(self.$index)),+];
                (
                    jobs,
                    Box::new(|outputs| {
                        let mut outputs = outputs.into_iter();
                        ($(
                            restore::<$value>(
                                outputs.next().expect("A concurrent task did not report a result."),
                            ),
                        )+)
                    }),
                )
            }
        }
    };
}

tuple_tasks!((F1, T1, 0));
tuple_tasks!((F1, T1, 0), (F2, T2, 1));
tuple_tasks!((F1, T1, 0), (F2, T2, 1), (F3, T3, 2));
tuple_tasks!((F1, T1, 0), (F2, T2, 1), (F3, T3, 2), (F4, T4, 3));
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4)
);
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4),
    (F6, T6, 5)
);
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4),
    (F6, T6, 5),
    (F7, T7, 6)
);
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4),
    (F6, T6, 5),
    (F7, T7, 6),
    (F8, T8, 7)
);
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4),
    (F6, T6, 5),
    (F7, T7, 6),
    (F8, T8, 7),
    (F9, T9, 8)
);
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4),
    (F6, T6, 5),
    (F7, T7, 6),
    (F8, T8, 7),
    (F9, T9, 8),
    (F10, T10, 9)
);
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4),
    (F6, T6, 5),
    (F7, T7, 6),
    (F8, T8, 7),
    (F9, T9, 8),
    (F10, T10, 9),
    (F11, T11, 10)
);
tuple_tasks!(
    (F1, T1, 0),
    (F2, T2, 1),
    (F3, T3, 2),
    (F4, T4, 3),
    (F5, T5, 4),
    (F6, T6, 5),
    (F7, T7, 6),
    (F8, T8, 7),
    (F9, T9, 8),
    (F10, T10, 9),
    (F11, T11, 10),
    (F12, T12, 11)
);

#[cfg(test)]
mod tests {
    use super::*;

    async fn run_inline<T: Tasks>(tasks: T) -> Result<T::Output> {
        let (jobs, assemble) = tasks.into_jobs();
        let mut outputs = Vec::new();
        for job in jobs {
            outputs.push(job.await?);
        }
        Ok(assemble(outputs))
    }

    #[tokio::test]
    async fn tuples_keep_their_types() {
        let (number, text) = run_inline((async { Ok(1) }, async { Ok("two") }))
            .await
            .unwrap();
        assert_eq!(number, 1);
        assert_eq!(text, "two");
    }

    #[tokio::test]
    async fn vectors_and_arrays_keep_their_order() {
        let values = run_inline(vec![task(async { Ok(1) }), task(async { Ok(2) })])
            .await
            .unwrap();
        assert_eq!(values, vec![1, 2]);

        let [a, b] = run_inline([task(async { Ok('a') }), task(async { Ok('b') })])
            .await
            .unwrap();
        assert_eq!((a, b), ('a', 'b'));
    }

    #[tokio::test]
    async fn maps_keep_their_keys() {
        let mut tasks = IndexMap::new();
        tasks.insert("users", task(async { Ok(10) }));
        tasks.insert("orders", task(async { Ok(20) }));

        let results = run_inline(tasks).await.unwrap();
        assert_eq!(results["users"], 10);
        assert_eq!(results["orders"], 20);
        assert_eq!(
            results.keys().copied().collect::<Vec<_>>(),
            vec!["users", "orders"]
        );
    }

    #[tokio::test]
    async fn twelve_tasks_fit_in_a_tuple() {
        let results = run_inline((
            async { Ok(1) },
            async { Ok(2) },
            async { Ok(3) },
            async { Ok(4) },
            async { Ok(5) },
            async { Ok(6) },
            async { Ok(7) },
            async { Ok(8) },
            async { Ok(9) },
            async { Ok(10) },
            async { Ok(11) },
            async { Ok("twelve") },
        ))
        .await
        .unwrap();
        assert_eq!(results.0, 1);
        assert_eq!(results.11, "twelve");
    }
}
