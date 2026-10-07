//! Pools of processes that run concurrently, and their results.

use std::ops::Index;
use std::sync::Arc;

use futures::future::join_all;
use illuminate_support::{Collection, Result};

use crate::command::{Command, IntoTimeout};
use crate::factory::Factory;
use crate::invoked::InvokedProcess;
use crate::output::OutputType;
use crate::pending::{EnvValue, PendingProcess};
use crate::result::ProcessResult;

/// Generate the methods that add a new pending process to a pool or pipe,
/// configured with a single option, so they read just like the facade.
macro_rules! pending_process_starters {
    () => {
        /// Add a process with the given key.
        ///
        /// The key identifies the process' result and is handed to output
        /// callbacks. Processes without a key are numbered `"0"`, `"1"`, ...
        pub fn as_(&mut self, key: impl Into<String>) -> &mut PendingProcess {
            let key = key.into();
            let pending = self.factory.new_pending_process();
            match self
                .processes
                .iter()
                .position(|(existing, _)| *existing == key)
            {
                Some(position) => {
                    self.processes[position].1 = pending;
                    &mut self.processes[position].1
                }
                None => {
                    self.processes.push((key, pending));
                    &mut self
                        .processes
                        .last_mut()
                        .expect("a process was just added")
                        .1
                }
            }
        }

        /// Add a process that runs the given command.
        pub fn command(&mut self, command: impl Into<Command>) -> &mut PendingProcess {
            self.add().command(command)
        }

        /// Add a process that runs in the given working directory.
        pub fn path(&mut self, path: impl Into<std::path::PathBuf>) -> &mut PendingProcess {
            self.add().path(path)
        }

        /// Add a process with the given timeout.
        pub fn timeout(&mut self, timeout: impl IntoTimeout) -> &mut PendingProcess {
            self.add().timeout(timeout)
        }

        /// Add a process with the given idle timeout.
        pub fn idle_timeout(&mut self, timeout: impl IntoTimeout) -> &mut PendingProcess {
            self.add().idle_timeout(timeout)
        }

        /// Add a process that may run forever.
        pub fn forever(&mut self) -> &mut PendingProcess {
            self.add().forever()
        }

        /// Add a process with the given environment variables.
        pub fn env<K, V>(
            &mut self,
            environment: impl IntoIterator<Item = (K, V)>,
        ) -> &mut PendingProcess
        where
            K: Into<String>,
            V: Into<EnvValue>,
        {
            self.add().env(environment)
        }

        /// Add a process with the given standard input.
        pub fn input(&mut self, input: impl Into<Vec<u8>>) -> &mut PendingProcess {
            self.add().input(input)
        }

        /// Add a process whose output is discarded.
        pub fn quietly(&mut self) -> &mut PendingProcess {
            self.add().quietly()
        }

        /// The number of processes that have been added.
        pub fn len(&self) -> usize {
            self.processes.len()
        }

        /// Determine if no processes have been added.
        pub fn is_empty(&self) -> bool {
            self.processes.is_empty()
        }

        /// Add a process under the next numeric key.
        fn add(&mut self) -> &mut PendingProcess {
            while self
                .processes
                .iter()
                .any(|(key, _)| *key == self.next_index.to_string())
            {
                self.next_index += 1;
            }
            let key = self.next_index.to_string();
            self.next_index += 1;
            self.as_(key)
        }
    };
}

pub(crate) use pending_process_starters;

/// A pool of processes, defined with
/// [`Process::pool`](crate::Process::pool), that run concurrently.
///
/// ```
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let mut pool = Process::pool(|pool| {
///     pool.as_("first").command("sleep 0.1; echo first");
///     pool.as_("second").command("echo second");
/// })
/// .start()?;
///
/// while !pool.running().is_empty() {
///     tokio::time::sleep(std::time::Duration::from_millis(10)).await;
/// }
///
/// let results = pool.wait().await?;
///
/// assert_eq!(results["first"].output(), "first\n");
/// assert_eq!(results["second"].output(), "second\n");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Debug)]
pub struct Pool {
    factory: Factory,
    processes: Vec<(String, PendingProcess)>,
    next_index: usize,
}

impl Pool {
    /// Create a new, empty pool for the given factory.
    pub fn new(factory: Factory) -> Self {
        Self {
            factory,
            processes: Vec::new(),
            next_index: 0,
        }
    }

    pending_process_starters!();

    /// Start every process in the pool.
    pub fn start(self) -> Result<InvokedProcessPool> {
        let processes = self
            .processes
            .into_iter()
            .map(|(key, pending)| Ok((key, pending.launch(None)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(InvokedProcessPool::new(processes))
    }

    /// Start every process in the pool, handing each line of output to the
    /// callback along with the key of the process that wrote it.
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let lines = Arc::new(Mutex::new(Vec::new()));
    /// let captured = lines.clone();
    ///
    /// Process::pool(|pool| {
    ///     pool.as_("first").command("echo one");
    ///     pool.as_("second").command("echo two");
    /// })
    /// .start_with_output(move |_, output, key| {
    ///     captured.lock().unwrap().push(format!("{key}: {output}"));
    /// })?
    /// .wait()
    /// .await?;
    ///
    /// let mut lines = lines.lock().unwrap().clone();
    /// lines.sort();
    /// assert_eq!(lines, ["first: one\n", "second: two\n"]);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn start_with_output<F>(self, output: F) -> Result<InvokedProcessPool>
    where
        F: Fn(OutputType, &str, &str) + Send + Sync + 'static,
    {
        let output = Arc::new(output);
        let processes = self
            .processes
            .into_iter()
            .map(|(key, pending)| {
                let output = output.clone();
                let handler_key = key.clone();
                let process = pending.launch(Some(Box::new(move |kind, line| {
                    output(kind, line, &handler_key)
                })))?;
                Ok((key, process))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(InvokedProcessPool::new(processes))
    }

    /// Start the processes and wait for them all to finish.
    pub async fn run(self) -> Result<ProcessPoolResults> {
        self.start()?.wait().await
    }

    /// Start the processes and wait for them all to finish.
    pub async fn wait(self) -> Result<ProcessPoolResults> {
        self.run().await
    }
}

/// A pool of processes that have been started.
#[derive(Debug)]
pub struct InvokedProcessPool {
    processes: Vec<(String, InvokedProcess)>,
}

impl InvokedProcessPool {
    /// Create a pool from processes that have already been started.
    pub fn new(processes: Vec<(String, InvokedProcess)>) -> Self {
        Self { processes }
    }

    /// Get the processes in the pool that are still running.
    pub fn running(&mut self) -> Vec<&mut InvokedProcess> {
        self.processes
            .iter_mut()
            .filter_map(|(_, process)| process.running().then_some(process))
            .collect()
    }

    /// Send a signal to each process in the pool that is still running,
    /// returning how many were signalled.
    pub fn signal(&mut self, signal: i32) -> Result<usize> {
        let mut running = self.running();
        for process in running.iter_mut() {
            process.signal(signal)?;
        }
        Ok(running.len())
    }

    /// Stop every process that is still running, returning how many were
    /// stopped.
    pub async fn stop(&mut self) -> usize {
        let running = self.running();
        let count = running.len();
        join_all(running.into_iter().map(|process| process.stop())).await;
        count
    }

    /// Wait for every process to finish.
    ///
    /// If any process times out, the first such error is returned once the
    /// others have finished.
    pub async fn wait(&mut self) -> Result<ProcessPoolResults> {
        let results = join_all(self.processes.iter_mut().map(|(_, process)| process.wait())).await;

        let results = self
            .processes
            .iter()
            .map(|(key, _)| key.clone())
            .zip(results)
            .map(|(key, result)| result.map(|result| (key, result)))
            .collect::<Result<Vec<_>>>()?;

        Ok(ProcessPoolResults::new(results))
    }

    /// The number of processes in the pool.
    pub fn len(&self) -> usize {
        self.processes.len()
    }

    /// Determine if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }

    /// Get the process with the given key.
    pub fn get(&self, key: &str) -> Option<&InvokedProcess> {
        self.processes
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, process)| process)
    }

    /// Get the process with the given key, mutably.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut InvokedProcess> {
        self.processes
            .iter_mut()
            .find(|(existing, _)| existing == key)
            .map(|(_, process)| process)
    }

    /// Iterate over the keys and processes.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &InvokedProcess)> {
        self.processes
            .iter()
            .map(|(key, process)| (key.as_str(), process))
    }

    /// Iterate over the keys and processes, mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut InvokedProcess)> {
        self.processes
            .iter_mut()
            .map(|(key, process)| (key.as_str(), process))
    }
}

/// The results of a pool of processes, accessible by key or position.
///
/// ```
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let [first, second, third] = Process::concurrently(|pool| {
///     pool.command("echo 1");
///     pool.command("echo 2");
///     pool.command("echo 3");
/// })
/// .await?
/// .into_array();
///
/// assert_eq!(first.output(), "1\n");
/// assert_eq!(second.output(), "2\n");
/// assert_eq!(third.output(), "3\n");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessPoolResults {
    results: Vec<(String, ProcessResult)>,
}

impl ProcessPoolResults {
    /// Create a result set from keyed results.
    pub fn new(results: Vec<(String, ProcessResult)>) -> Self {
        Self { results }
    }

    /// Determine if every process in the pool was successful.
    pub fn successful(&self) -> bool {
        self.results.iter().all(|(_, result)| result.successful())
    }

    /// Determine if any process in the pool failed.
    pub fn failed(&self) -> bool {
        !self.successful()
    }

    /// Get the result with the given key.
    pub fn get(&self, key: &str) -> Option<&ProcessResult> {
        self.results
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, result)| result)
    }

    /// Determine if there is a result with the given key.
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// The keys of the results, in order.
    pub fn keys(&self) -> Vec<&str> {
        self.results.iter().map(|(key, _)| key.as_str()).collect()
    }

    /// Get the results as a collection.
    pub fn collect(&self) -> Collection<ProcessResult> {
        Collection::make(self.results.iter().map(|(_, result)| result.clone()))
    }

    /// Iterate over the keys and results.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ProcessResult)> {
        self.results
            .iter()
            .map(|(key, result)| (key.as_str(), result))
    }

    /// The number of results.
    pub fn len(&self) -> usize {
        self.results.len()
    }

    /// Determine if there are no results.
    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    /// The results, in order.
    pub fn into_vec(self) -> Vec<ProcessResult> {
        self.results.into_iter().map(|(_, result)| result).collect()
    }

    /// The results as an array, ready for destructuring.
    ///
    /// # Panics
    ///
    /// Panics if the pool doesn't contain exactly `N` results.
    pub fn into_array<const N: usize>(self) -> [ProcessResult; N] {
        let count = self.results.len();
        self.into_vec()
            .try_into()
            .unwrap_or_else(|_| panic!("Expected {N} process results, but the pool has {count}."))
    }
}

impl Index<usize> for ProcessPoolResults {
    type Output = ProcessResult;

    fn index(&self, position: usize) -> &ProcessResult {
        &self.results[position].1
    }
}

impl Index<&str> for ProcessPoolResults {
    type Output = ProcessResult;

    fn index(&self, key: &str) -> &ProcessResult {
        self.get(key)
            .unwrap_or_else(|| panic!("Undefined process pool key [{key}]."))
    }
}

impl IntoIterator for ProcessPoolResults {
    type Item = (String, ProcessResult);
    type IntoIter = std::vec::IntoIter<(String, ProcessResult)>;

    fn into_iter(self) -> Self::IntoIter {
        self.results.into_iter()
    }
}

impl<'a> IntoIterator for &'a ProcessPoolResults {
    type Item = &'a (String, ProcessResult);
    type IntoIter = std::slice::Iter<'a, (String, ProcessResult)>;

    fn into_iter(self) -> Self::IntoIter {
        self.results.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results() -> ProcessPoolResults {
        ProcessPoolResults::new(vec![
            ("first".into(), ProcessResult::new("a", Some(0), "a\n", "")),
            ("second".into(), ProcessResult::new("b", Some(1), "b\n", "")),
        ])
    }

    #[test]
    fn results_are_accessible_by_key_and_position() {
        let results = results();
        assert_eq!(results[0].output(), "a\n");
        assert_eq!(results["second"].output(), "b\n");
        assert!(results.has("first"));
        assert!(!results.has("third"));
        assert_eq!(results.keys(), vec!["first", "second"]);
        assert_eq!(results.len(), 2);
        assert!(!results.is_empty());
        assert_eq!(results.collect().count(), 2);
        assert_eq!(
            results.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!((&results).into_iter().count(), 2);
    }

    #[test]
    fn results_know_if_any_process_failed() {
        let results = results();
        assert!(results.failed());
        assert!(!results.successful());
        assert!(ProcessPoolResults::default().successful());
    }

    #[test]
    fn results_can_be_destructured() {
        let [first, second] = results().into_array();
        assert_eq!(first.command(), "a");
        assert_eq!(second.command(), "b");

        let keyed: Vec<(String, ProcessResult)> = results().into_iter().collect();
        assert_eq!(keyed[1].0, "second");
    }

    #[test]
    #[should_panic(expected = "Expected 3 process results, but the pool has 2.")]
    fn destructuring_the_wrong_number_of_results_panics() {
        let [_, _, _] = results().into_array();
    }

    #[test]
    #[should_panic(expected = "Undefined process pool key [missing].")]
    fn missing_keys_panic() {
        let _ = &results()["missing"];
    }

    #[test]
    fn unnamed_processes_are_numbered() {
        let mut pool = Pool::new(Factory::new());
        pool.command("a");
        pool.as_("named").command("b");
        pool.command("c");
        pool.as_("2").command("d");
        pool.command("e");
        let keys: Vec<&str> = pool.processes.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, vec!["0", "named", "1", "2", "3"]);
        assert_eq!(pool.len(), 5);
    }

    #[test]
    fn keys_may_be_reused_to_replace_a_process() {
        let mut pool = Pool::new(Factory::new());
        pool.as_("first").command("a");
        pool.as_("first").command("b");
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.processes[0].1.command, Some(Command::from("b")));
    }

    #[test]
    fn pool_starters_configure_new_processes() {
        let mut pool = Pool::new(Factory::new());
        pool.path("/tmp").command("ls");
        pool.timeout(5).command("ls");
        pool.idle_timeout(5).command("ls");
        pool.forever().command("ls");
        pool.env([("A", "1")]).command("ls");
        pool.input("x").command("ls");
        pool.quietly().command("ls");
        assert_eq!(pool.len(), 7);
        assert!(!pool.is_empty());
        assert_eq!(pool.processes[3].1.timeout, None);
        assert!(pool.processes[6].1.quietly);
    }
}
