//! # Illuminate Pipeline
//!
//! A pipeline is a convenient way to "pipe" a value through a series of
//! stages, giving each stage the opportunity to inspect or modify the value
//! and then hand it to the next one — exactly like HTTP middleware, but for
//! anything you like.
//!
//! ```
//! use illuminate_pipeline::{Next, Pipeline};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # runtime.block_on(async {
//! let greeting = Pipeline::send(String::from("taylor"))
//!     .through((
//!         |name: String, next: Next<String>| async move { next.run(name.to_uppercase()).await },
//!         |name: String, next: Next<String>| async move { next.run(format!("Hello, {name}!")).await },
//!     ))
//!     .then_return()
//!     .await;
//!
//! assert_eq!(greeting, "Hello, TAYLOR!");
//! # });
//! ```
//!
//! Stages may be closures or any type implementing [`Pipe`]:
//!
//! ```
//! use illuminate_pipeline::{async_trait, Next, Pipe, Pipeline};
//!
//! struct User {
//!     name: String,
//!     active: bool,
//! }
//!
//! struct ActivateSubscription;
//!
//! #[async_trait]
//! impl Pipe<User> for ActivateSubscription {
//!     async fn handle(&self, mut user: User, next: Next<User>) -> User {
//!         user.active = true;
//!         next.run(user).await
//!     }
//! }
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # runtime.block_on(async {
//! let user = Pipeline::send(User { name: "Taylor".into(), active: false })
//!     .through((ActivateSubscription,))
//!     .then_return()
//!     .await;
//!
//! assert!(user.active);
//! # });
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use illuminate_container::ServiceProvider;
use illuminate_support::Conditionable;

/// Re-exported so implementors of [`Pipe`] don't need their own dependency.
pub use async_trait::async_trait;

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A single stage of a [`Pipeline`].
///
/// A pipe receives the passable value and the [`Next`] stage. It may change
/// the value before passing it along, inspect the result on the way back
/// out, or return early without calling `next` at all.
///
/// Closures of the form `|value: T, next: Next<T, R>| async move { ... }`
/// are pipes too.
#[async_trait]
pub trait Pipe<T, R = T>: Send + Sync + 'static
where
    T: Send + 'static,
    R: Send + 'static,
{
    /// Handle the passable value.
    async fn handle(&self, passable: T, next: Next<T, R>) -> R;
}

/// Closures make perfectly good pipes.
#[async_trait]
impl<T, R, F, Fut> Pipe<T, R> for F
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T, Next<T, R>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
{
    async fn handle(&self, passable: T, next: Next<T, R>) -> R {
        (self)(passable, next).await
    }
}

type Continuation<T, R> = Box<dyn FnOnce(T) -> BoxFuture<'static, R> + Send>;

type Finally<R> = Box<dyn FnOnce(&R) + Send>;

/// The rest of the pipeline.
///
/// Calling [`Next::run`] hands the value to the next stage (and, eventually,
/// to the pipeline's destination). It consumes the continuation, so the
/// remainder of the pipeline runs at most once.
pub struct Next<T, R = T> {
    inner: Continuation<T, R>,
}

impl<T, R> Next<T, R> {
    /// Pass the value to the next stage of the pipeline.
    pub async fn run(self, passable: T) -> R {
        (self.inner)(passable).await
    }
}

impl<T, R> std::fmt::Debug for Next<T, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Next")
    }
}

/// Collections of pipes accepted by [`Pipeline::through`].
///
/// Implemented for tuples of [`Pipe`]s (so differently typed stages can be
/// mixed freely), and for vectors and arrays of `Arc<dyn Pipe<T, R>>`.
pub trait IntoPipes<T, R> {
    /// Convert into a list of shared pipes.
    fn into_pipes(self) -> Vec<Arc<dyn Pipe<T, R>>>;
}

impl<T, R> IntoPipes<T, R> for Vec<Arc<dyn Pipe<T, R>>> {
    fn into_pipes(self) -> Vec<Arc<dyn Pipe<T, R>>> {
        self
    }
}

impl<T, R, const N: usize> IntoPipes<T, R> for [Arc<dyn Pipe<T, R>>; N] {
    fn into_pipes(self) -> Vec<Arc<dyn Pipe<T, R>>> {
        self.into_iter().collect()
    }
}

impl<T, R> IntoPipes<T, R> for () {
    fn into_pipes(self) -> Vec<Arc<dyn Pipe<T, R>>> {
        Vec::new()
    }
}

macro_rules! tuple_pipes {
    ($($name:ident),+) => {
        impl<T, R, $($name),+> IntoPipes<T, R> for ($($name,)+)
        where
            T: Send + 'static,
            R: Send + 'static,
            $($name: Pipe<T, R>,)+
        {
            #[allow(non_snake_case)]
            fn into_pipes(self) -> Vec<Arc<dyn Pipe<T, R>>> {
                let ($($name,)+) = self;
                vec![$(Arc::new($name) as Arc<dyn Pipe<T, R>>),+]
            }
        }
    };
}

tuple_pipes!(A);
tuple_pipes!(A, B);
tuple_pipes!(A, B, C);
tuple_pipes!(A, B, C, D);
tuple_pipes!(A, B, C, D, E);
tuple_pipes!(A, B, C, D, E, F);
tuple_pipes!(A, B, C, D, E, F, G);
tuple_pipes!(A, B, C, D, E, F, G, H);
tuple_pipes!(A, B, C, D, E, F, G, H, I);
tuple_pipes!(A, B, C, D, E, F, G, H, I, J);
tuple_pipes!(A, B, C, D, E, F, G, H, I, J, K);
tuple_pipes!(A, B, C, D, E, F, G, H, I, J, K, L);

/// Send a value through a series of pipes.
///
/// `T` is the value being sent through the pipeline and `R` is what the
/// pipeline ultimately returns (the passable itself, by default).
pub struct Pipeline<T, R = T> {
    passable: T,
    pipes: Vec<Arc<dyn Pipe<T, R>>>,
    finally: Option<Finally<R>>,
}

impl<T, R> std::fmt::Debug for Pipeline<T, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("pipes", &self.pipes.len())
            .finish()
    }
}

impl<T, R> Pipeline<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    /// Set the value being sent through the pipeline.
    pub fn send(passable: T) -> Self {
        Self {
            passable,
            pipes: Vec::new(),
            finally: None,
        }
    }

    /// Set the pipes the value will travel through, replacing any others.
    pub fn through(mut self, pipes: impl IntoPipes<T, R>) -> Self {
        self.pipes = pipes.into_pipes();
        self
    }

    /// Push an additional pipe onto the end of the pipeline.
    ///
    /// ```
    /// use illuminate_pipeline::{Next, Pipeline};
    ///
    /// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// # runtime.block_on(async {
    /// let total = Pipeline::send(1)
    ///     .pipe(|n: i32, next: Next<i32>| async move { next.run(n + 1).await })
    ///     .pipe(|n: i32, next: Next<i32>| async move { next.run(n * 10).await })
    ///     .then_return()
    ///     .await;
    ///
    /// assert_eq!(total, 20);
    /// # });
    /// ```
    pub fn pipe(mut self, pipe: impl Pipe<T, R>) -> Self {
        self.pipes.push(Arc::new(pipe));
        self
    }

    /// Push many additional pipes onto the end of the pipeline.
    pub fn pipes(mut self, pipes: impl IntoPipes<T, R>) -> Self {
        self.pipes.extend(pipes.into_pipes());
        self
    }

    /// Register a callback to run once the pipeline has finished. It
    /// receives the pipeline's result.
    pub fn finally(mut self, callback: impl FnOnce(&R) + Send + 'static) -> Self {
        self.finally = Some(Box::new(callback));
        self
    }

    /// The number of pipes in the pipeline.
    pub fn len(&self) -> usize {
        self.pipes.len()
    }

    /// Determine if the pipeline has no pipes.
    pub fn is_empty(&self) -> bool {
        self.pipes.is_empty()
    }

    /// Run the pipeline with a final destination callback.
    ///
    /// ```
    /// use illuminate_pipeline::{Next, Pipeline};
    ///
    /// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// # runtime.block_on(async {
    /// let length = Pipeline::send(String::from("  Laravel  "))
    ///     .through((|s: String, next: Next<String, usize>| async move {
    ///         next.run(s.trim().to_string()).await
    ///     },))
    ///     .then(|s| async move { s.len() })
    ///     .await;
    ///
    /// assert_eq!(length, 7);
    /// # });
    /// ```
    pub async fn then<F, Fut>(self, destination: F) -> R
    where
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        let destination: Continuation<T, R> =
            Box::new(move |passable| Box::pin(destination(passable)) as BoxFuture<'static, R>);

        let pipeline = self
            .pipes
            .into_iter()
            .rev()
            .fold(destination, |next, pipe| {
                Box::new(move |passable: T| {
                    Box::pin(async move { pipe.handle(passable, Next { inner: next }).await })
                        as BoxFuture<'static, R>
                })
            });

        let result = pipeline(self.passable).await;

        if let Some(finally) = self.finally {
            finally(&result);
        }

        result
    }
}

impl<T> Pipeline<T, T>
where
    T: Send + 'static,
{
    /// Run the pipeline and return the (possibly modified) passable.
    pub async fn then_return(self) -> T {
        self.then(|passable| async move { passable }).await
    }
}

impl<T, R> Conditionable for Pipeline<T, R> {}

/// The pipeline service provider.
///
/// Pipelines are lightweight values created on demand with
/// [`Pipeline::send`], so there is nothing to bind into the container; the
/// provider exists so the application's provider list mirrors Laravel's.
#[derive(Debug, Default, Clone, Copy)]
pub struct PipelineServiceProvider;

impl ServiceProvider for PipelineServiceProvider {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn pipes_run_in_order_around_the_destination() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (a, b, d) = (log.clone(), log.clone(), log.clone());

        let result = Pipeline::send(1)
            .through((
                move |n: i32, next: Next<i32>| {
                    let log = a.clone();
                    async move {
                        log.lock().unwrap().push("first:before");
                        let result = next.run(n + 1).await;
                        log.lock().unwrap().push("first:after");
                        result
                    }
                },
                move |n: i32, next: Next<i32>| {
                    let log = b.clone();
                    async move {
                        log.lock().unwrap().push("second");
                        next.run(n * 10).await
                    }
                },
            ))
            .then(move |n| async move {
                d.lock().unwrap().push("destination");
                n + 5
            })
            .await;

        assert_eq!(result, 25);
        assert_eq!(
            *log.lock().unwrap(),
            vec!["first:before", "second", "destination", "first:after"]
        );
    }

    #[tokio::test]
    async fn then_return_gives_back_the_passable() {
        let value = Pipeline::send(vec![1, 2])
            .pipe(|mut items: Vec<i32>, next: Next<Vec<i32>>| async move {
                items.push(3);
                next.run(items).await
            })
            .then_return()
            .await;
        assert_eq!(value, vec![1, 2, 3]);
    }

    struct Prefix(&'static str);

    #[async_trait]
    impl Pipe<String> for Prefix {
        async fn handle(&self, value: String, next: Next<String>) -> String {
            next.run(format!("{}{value}", self.0)).await
        }
    }

    struct Shout;

    #[async_trait]
    impl Pipe<String> for Shout {
        async fn handle(&self, value: String, next: Next<String>) -> String {
            next.run(value.to_uppercase()).await
        }
    }

    #[tokio::test]
    async fn types_implementing_pipe_can_be_mixed_with_closures() {
        let value = Pipeline::send("taylor".to_string())
            .through((
                Prefix("hello "),
                Shout,
                |value: String, next: Next<String>| async move { next.run(value + "!").await },
            ))
            .then_return()
            .await;
        assert_eq!(value, "HELLO TAYLOR!");
    }

    #[tokio::test]
    async fn pipes_may_short_circuit() {
        let reached = Arc::new(Mutex::new(false));
        let flag = reached.clone();
        let value = Pipeline::send(5)
            .through((|n: i32, _next: Next<i32>| async move { n * 100 },))
            .then(move |n| async move {
                *flag.lock().unwrap() = true;
                n
            })
            .await;
        assert_eq!(value, 500);
        assert!(!*reached.lock().unwrap());
    }

    #[tokio::test]
    async fn result_pipelines_stop_at_the_first_error() {
        let outcome: Result<i32, String> = Pipeline::send(3)
            .through((|n: i32, next: Next<i32, Result<i32, String>>| async move {
                if n < 10 {
                    return Err(format!("{n} is too small"));
                }
                next.run(n).await
            },))
            .then(|n| async move { Ok(n) })
            .await;
        assert_eq!(outcome.unwrap_err(), "3 is too small");
    }

    #[tokio::test]
    async fn through_replaces_and_pipe_appends() {
        let value = Pipeline::send(String::new())
            .through((Prefix("a"),))
            .through((Prefix("b"),))
            .pipe(Prefix("c"))
            .pipes((Prefix("d"), Prefix("e")))
            .then_return()
            .await;
        assert_eq!(value, "edcb");
    }

    #[tokio::test]
    async fn vectors_of_shared_pipes_are_accepted() {
        let pipes: Vec<Arc<dyn Pipe<String>>> = vec![Arc::new(Prefix("x")), Arc::new(Shout)];
        let pipeline = Pipeline::send("y".to_string()).through(pipes);
        assert_eq!(pipeline.len(), 2);
        assert_eq!(pipeline.then_return().await, "XY");
    }

    #[tokio::test]
    async fn finally_receives_the_result() {
        let seen = Arc::new(Mutex::new(None));
        let sink = seen.clone();
        let value = Pipeline::send(41)
            .pipe(|n: i32, next: Next<i32>| async move { next.run(n + 1).await })
            .finally(move |result| *sink.lock().unwrap() = Some(*result))
            .then_return()
            .await;
        assert_eq!(value, 42);
        assert_eq!(*seen.lock().unwrap(), Some(42));
    }

    #[tokio::test]
    async fn pipelines_are_conditionable() {
        let shout = false;
        let value = Pipeline::send("quiet".to_string())
            .when(shout, |pipeline| pipeline.pipe(Shout))
            .unless(shout, |pipeline| pipeline.pipe(Prefix("very ")))
            .then_return()
            .await;
        assert_eq!(value, "very quiet");
    }

    #[tokio::test]
    async fn an_empty_pipeline_goes_straight_to_the_destination() {
        let pipeline = Pipeline::send(7).through(());
        assert!(pipeline.is_empty());
        assert_eq!(pipeline.then(|n| async move { n * 6 }).await, 42);
    }

    #[tokio::test]
    async fn pipelines_run_on_multi_threaded_runtimes() {
        let handle = tokio::spawn(async {
            Pipeline::send(1)
                .pipe(|n: i32, next: Next<i32>| async move {
                    tokio::task::yield_now().await;
                    next.run(n + 1).await
                })
                .then_return()
                .await
        });
        assert_eq!(handle.await.unwrap(), 2);
    }
}
