//! Pruning models that are no longer needed.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use illuminate_support::Result;

use super::builder::Builder;
use super::model::Model;
use super::state::EloquentState;

/// A callback told the running total of pruned models after each chunk.
pub(crate) type Progress = Arc<dyn Fn(u64) + Send + Sync>;

/// Periodically delete models that are no longer needed, one at a time.
///
/// Implement [`prunable`](Prunable::prunable) to return a query for the
/// models to prune. [`prune_all`](Prunable::prune_all) retrieves them in
/// chunks and deletes each one, so the `deleting` / `deleted` events fire
/// and [`pruning`](Prunable::pruning) can clean up after the model first.
/// Soft deletable models are permanently deleted.
///
/// ```
/// use illuminate_database::eloquent::*;
/// use illuminate_support::Result;
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// # use illuminate_database::{DatabaseManager, Schema};
///
/// #[derive(Debug, Clone, Default, Model)]
/// #[fillable(name, arrived)]
/// pub struct Flight {
///     pub id: u64,
///     pub name: String,
///     pub arrived: bool,
/// }
///
/// impl Prunable for Flight {
///     fn prunable() -> Builder<Self> {
///         Flight::where_("arrived", true)
///     }
///
///     async fn pruning(&mut self) -> Result<()> {
///         println!("Archiving the manifest of flight {}...", self.name);
///         Ok(())
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<()> {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container.clone());
/// # container.instance(DatabaseManager::from_config(json!({
/// #     "default": "sqlite",
/// #     "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
/// # })));
/// # Schema::create("flights", |table| {
/// #     table.id();
/// #     table.string("name");
/// #     table.boolean("arrived");
/// # }).await?;
/// Flight::create(json!({"name": "Oceanic 815", "arrived": false})).await?;
/// Flight::create(json!({"name": "Ajira 316", "arrived": true})).await?;
///
/// assert_eq!(Flight::prune_all(1000).await?, 1);
/// assert_eq!(Flight::count().await?, 1);
/// # Ok(())
/// # }
/// ```
///
/// Every `#[derive(Model)]` model implementing `Prunable` is discovered by
/// `php artisan model:prune` (see [`registry`](super::registry)).
pub trait Prunable: Model {
    /// The query matching the models to prune.
    fn prunable() -> Builder<Self>;

    /// Prepare the model for pruning: called right before it is deleted.
    fn pruning(&mut self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Prune the model: run [`pruning`](Prunable::pruning), then delete it
    /// (permanently, for soft deletable models).
    fn prune(&mut self) -> impl Future<Output = Result<bool>> + Send {
        async move {
            self.pruning().await?;
            if Self::soft_deletes() {
                self.force_delete().await
            } else {
                self.delete().await
            }
        }
    }

    /// Prune every prunable model, retrieving `chunk_size` models at a
    /// time. Returns how many were pruned.
    fn prune_all(chunk_size: u64) -> impl Future<Output = Result<u64>> + Send {
        prune_each::<Self>(chunk_size, None)
    }
}

/// Periodically delete models that are no longer needed, with mass delete
/// queries.
///
/// The models are never retrieved, so pruning is much more efficient — but
/// no `pruning` hook runs and no model events fire. Soft deletable models
/// are permanently deleted.
///
/// ```ignore
/// impl MassPrunable for Flight {
///     fn prunable() -> Builder<Self> {
///         Flight::where_op("created_at", "<=", Carbon::now().sub_months(1))
///     }
/// }
///
/// let pruned = Flight::prune_all(1000).await?;
/// ```
pub trait MassPrunable: Model {
    /// The query matching the models to prune.
    fn prunable() -> Builder<Self>;

    /// Prune every prunable model, deleting up to `chunk_size` rows per
    /// query (unless the prunable query has its own limit). Returns how many
    /// were pruned.
    fn prune_all(chunk_size: u64) -> impl Future<Output = Result<u64>> + Send {
        mass_prune::<Self>(chunk_size, None)
    }
}

/// How a model is pruned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum PruneKind {
    /// [`Prunable`]: models are retrieved and deleted one by one.
    Prunable,
    /// [`MassPrunable`]: models are deleted with mass delete queries.
    MassPrunable,
}

impl PruneKind {
    /// The kind's name, as Laravel spells the trait.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Prunable => "Prunable",
            Self::MassPrunable => "MassPrunable",
        }
    }
}

/// The prunable query, including soft deleted models.
fn prunable_query<M: Model>(query: Builder<M>) -> Builder<M> {
    if M::soft_deletes() {
        query.with_trashed()
    } else {
        query
    }
}

/// How many models a prune would delete (`model:prune --pretend`).
pub(crate) async fn pretend<M: Model>(query: Builder<M>) -> Result<u64> {
    Ok(prunable_query(query).count().await?.max(0) as u64)
}

/// Prune a [`Prunable`] model's records one model at a time.
pub(crate) async fn prune_each<M: Prunable>(
    chunk_size: u64,
    progress: Option<Progress>,
) -> Result<u64> {
    let total = Arc::new(AtomicU64::new(0));
    let reporter = EloquentState::resolve().reporter();
    let counter = total.clone();
    prunable_query(M::prunable())
        .chunk_by_id(chunk_size.max(1) as i64, move |models, _| {
            let (total, reporter, progress) = (counter.clone(), reporter.clone(), progress.clone());
            async move {
                for mut model in models {
                    match model.prune().await {
                        Ok(true) => {
                            total.fetch_add(1, Ordering::SeqCst);
                        }
                        Ok(false) => {}
                        Err(error) => match &reporter {
                            Some(report) => report(&error),
                            None => return Err(error),
                        },
                    }
                }
                if let Some(progress) = &progress {
                    progress(total.load(Ordering::SeqCst));
                }
                Ok(true)
            }
        })
        .await?;
    Ok(total.load(Ordering::SeqCst))
}

/// Prune a [`MassPrunable`] model's records with mass delete queries.
pub(crate) async fn mass_prune<M: MassPrunable>(
    chunk_size: u64,
    progress: Option<Progress>,
) -> Result<u64> {
    let mut query = prunable_query(M::prunable());
    if query.get_query().limit.is_none() {
        query = query.limit(chunk_size.max(1) as i64);
    }
    let mut total = 0;
    loop {
        let count = query.force_delete().await?;
        if count == 0 {
            break;
        }
        total += count;
        if let Some(progress) = &progress {
            progress(total);
        }
    }
    Ok(total)
}
