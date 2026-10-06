//! Quickly measure how long code takes to run.
//!
//! ```
//! use illuminate_support::Benchmark;
//!
//! let milliseconds = Benchmark::measure(|| { let _ = (1..1000).sum::<u64>(); });
//! assert!(milliseconds >= 0.0);
//!
//! let (sum, milliseconds) = Benchmark::value(|| (1..=10).sum::<u64>());
//! assert_eq!(sum, 55);
//! assert!(milliseconds >= 0.0);
//! ```

use std::future::Future;
use std::time::Instant;

use indexmap::IndexMap;

/// Static benchmarking helpers, mirroring `Illuminate\Support\Benchmark`.
pub struct Benchmark;

impl Benchmark {
    /// Measure how long the callback takes, in milliseconds.
    pub fn measure(callback: impl FnMut()) -> f64 {
        Self::measure_iterations(callback, 1)
    }

    /// Measure the average time of `iterations` runs, in milliseconds.
    pub fn measure_iterations(mut callback: impl FnMut(), iterations: usize) -> f64 {
        let iterations = iterations.max(1);
        let total: f64 = (0..iterations)
            .map(|_| {
                let start = Instant::now();
                callback();
                start.elapsed().as_secs_f64() * 1000.0
            })
            .sum();
        total / iterations as f64
    }

    /// Measure several named callbacks, returning the average milliseconds of each.
    ///
    /// ```
    /// use illuminate_support::Benchmark;
    ///
    /// let results = Benchmark::measure_many(
    ///     vec![
    ///         ("Scenario 1", Box::new(|| { let _ = 1 + 1; }) as Box<dyn FnMut()>),
    ///         ("Scenario 2", Box::new(|| { let _ = 2 + 2; })),
    ///     ],
    ///     10,
    /// );
    /// assert_eq!(results.keys().collect::<Vec<_>>(), vec!["Scenario 1", "Scenario 2"]);
    /// ```
    pub fn measure_many<'a>(
        benchmarks: impl IntoIterator<Item = (&'a str, Box<dyn FnMut() + 'a>)>,
        iterations: usize,
    ) -> IndexMap<String, f64> {
        benchmarks
            .into_iter()
            .map(|(name, callback)| (name.to_string(), Self::measure_iterations(callback, iterations)))
            .collect()
    }

    /// Run the callback, returning its value and how long it took (in milliseconds).
    pub fn value<T>(callback: impl FnOnce() -> T) -> (T, f64) {
        let start = Instant::now();
        let value = callback();
        (value, start.elapsed().as_secs_f64() * 1000.0)
    }

    /// Measure how long a future takes to complete, in milliseconds.
    pub async fn measure_async<F: Future>(future: F) -> f64 {
        Self::value_async(future).await.1
    }

    /// Await the future, returning its output and how long it took (in milliseconds).
    pub async fn value_async<F: Future>(future: F) -> (F::Output, f64) {
        let start = Instant::now();
        let value = future.await;
        (value, start.elapsed().as_secs_f64() * 1000.0)
    }

    /// Measure the callback and print the result (like Laravel's `Benchmark::dd`,
    /// without exiting), returning the formatted measurement.
    pub fn dump(callback: impl FnMut(), iterations: usize) -> String {
        let formatted = format!("{:.3}ms", Self::measure_iterations(callback, iterations));
        eprintln!("{formatted}");
        formatted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn it_measures() {
        let elapsed = Benchmark::measure(|| std::thread::sleep(Duration::from_millis(5)));
        assert!(elapsed >= 5.0);
        let mut runs = 0;
        Benchmark::measure_iterations(|| runs += 1, 3);
        assert_eq!(runs, 3);
        assert!(Benchmark::dump(|| {}, 1).ends_with("ms"));
    }

    #[tokio::test]
    async fn it_measures_futures() {
        let (value, elapsed) = Benchmark::value_async(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            "done"
        })
        .await;
        assert_eq!(value, "done");
        assert!(elapsed >= 5.0);
        assert!(Benchmark::measure_async(async {}).await >= 0.0);
    }
}
