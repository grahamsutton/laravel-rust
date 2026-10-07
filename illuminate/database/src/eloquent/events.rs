//! Model events: observers and closure listeners.

use std::sync::Arc;

use async_trait::async_trait;
use illuminate_support::Result;

use super::model::Model;
use super::state::{EloquentState, events_muted};

/// The events a model fires during its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelEvent {
    /// A model was hydrated from the database.
    Retrieved,
    /// A new model is about to be inserted (halting).
    Creating,
    /// A new model was inserted.
    Created,
    /// An existing model is about to be updated (halting).
    Updating,
    /// An existing model was updated.
    Updated,
    /// A model is about to be inserted or updated (halting).
    Saving,
    /// A model was inserted or updated.
    Saved,
    /// A model is about to be deleted (halting).
    Deleting,
    /// A model was deleted.
    Deleted,
    /// A soft deleted model is about to be restored (halting).
    Restoring,
    /// A soft deleted model was restored.
    Restored,
    /// A model is about to be permanently deleted (halting).
    ForceDeleting,
    /// A model was permanently deleted.
    ForceDeleted,
    /// A model was soft deleted.
    Trashed,
    /// A model is being replicated.
    Replicating,
}

impl ModelEvent {
    /// The event's name, as Laravel spells it (`creating`, `forceDeleted`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Retrieved => "retrieved",
            Self::Creating => "creating",
            Self::Created => "created",
            Self::Updating => "updating",
            Self::Updated => "updated",
            Self::Saving => "saving",
            Self::Saved => "saved",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
            Self::Restoring => "restoring",
            Self::Restored => "restored",
            Self::ForceDeleting => "forceDeleting",
            Self::ForceDeleted => "forceDeleted",
            Self::Trashed => "trashed",
            Self::Replicating => "replicating",
        }
    }

    /// Whether listeners may cancel the operation by returning `false`.
    pub fn is_halting(&self) -> bool {
        matches!(
            self,
            Self::Creating
                | Self::Updating
                | Self::Saving
                | Self::Deleting
                | Self::Restoring
                | Self::ForceDeleting
        )
    }
}

/// A class that listens to many events of one model.
///
/// Every method has a default, so observers only implement the events they
/// care about. The "-ing" events may cancel the operation by returning
/// `Ok(false)`.
///
/// ```ignore
/// struct UserObserver;
///
/// #[async_trait]
/// impl Observer<User> for UserObserver {
///     async fn creating(&self, user: &mut User) -> Result<bool> {
///         user.name = user.name.trim().to_string();
///         Ok(true)
///     }
///
///     async fn created(&self, user: &mut User) -> Result<()> {
///         Profile::create(json!({"user_id": user.id})).await?;
///         Ok(())
///     }
/// }
///
/// User::observe(UserObserver);
/// ```
#[async_trait]
#[allow(unused_variables)]
pub trait Observer<M: Model>: Send + Sync + 'static {
    /// A model was hydrated from the database.
    async fn retrieved(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// A new model is about to be inserted.
    async fn creating(&self, model: &mut M) -> Result<bool> {
        Ok(true)
    }

    /// A new model was inserted.
    async fn created(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// An existing model is about to be updated.
    async fn updating(&self, model: &mut M) -> Result<bool> {
        Ok(true)
    }

    /// An existing model was updated.
    async fn updated(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// A model is about to be saved.
    async fn saving(&self, model: &mut M) -> Result<bool> {
        Ok(true)
    }

    /// A model was saved.
    async fn saved(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// A model is about to be deleted.
    async fn deleting(&self, model: &mut M) -> Result<bool> {
        Ok(true)
    }

    /// A model was deleted.
    async fn deleted(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// A soft deleted model is about to be restored.
    async fn restoring(&self, model: &mut M) -> Result<bool> {
        Ok(true)
    }

    /// A soft deleted model was restored.
    async fn restored(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// A model is about to be permanently deleted.
    async fn force_deleting(&self, model: &mut M) -> Result<bool> {
        Ok(true)
    }

    /// A model was permanently deleted.
    async fn force_deleted(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// A model was soft deleted.
    async fn trashed(&self, model: &mut M) -> Result<()> {
        Ok(())
    }

    /// A model is being replicated.
    async fn replicating(&self, model: &mut M) -> Result<()> {
        Ok(())
    }
}

/// What a closure listener may return: nothing, a `bool` (`false` cancels a
/// halting event), or a `Result` of either.
pub trait EventOutcome {
    /// Whether the operation should proceed.
    fn into_outcome(self) -> Result<bool>;
}

impl EventOutcome for () {
    fn into_outcome(self) -> Result<bool> {
        Ok(true)
    }
}

impl EventOutcome for bool {
    fn into_outcome(self) -> Result<bool> {
        Ok(self)
    }
}

impl EventOutcome for Result<()> {
    fn into_outcome(self) -> Result<bool> {
        self.map(|_| true)
    }
}

impl EventOutcome for Result<bool> {
    fn into_outcome(self) -> Result<bool> {
        self
    }
}

type ClosureListener<M> = Arc<dyn Fn(&mut M) -> Result<bool> + Send + Sync>;

/// A registered listener.
pub(crate) enum Listener<M: Model> {
    Closure(ModelEvent, ClosureListener<M>),
    Observer(Arc<dyn Observer<M>>),
}

impl<M: Model> Clone for Listener<M> {
    fn clone(&self) -> Self {
        match self {
            Self::Closure(event, callback) => Self::Closure(*event, callback.clone()),
            Self::Observer(observer) => Self::Observer(observer.clone()),
        }
    }
}

/// Register a closure listener for a model event.
pub(crate) fn listen<M: Model, O: EventOutcome>(
    event: ModelEvent,
    callback: impl Fn(&mut M) -> O + Send + Sync + 'static,
) {
    let state = EloquentState::resolve();
    state.boot::<M>();
    state.add_listener::<M>(Listener::Closure(
        event,
        Arc::new(move |model| callback(model).into_outcome()),
    ));
}

/// Register an observer.
pub(crate) fn observe<M: Model>(observer: impl Observer<M>) {
    let state = EloquentState::resolve();
    state.add_listener::<M>(Listener::Observer(Arc::new(observer)));
}

async fn dispatch<M: Model>(
    observer: &dyn Observer<M>,
    event: ModelEvent,
    model: &mut M,
) -> Result<bool> {
    let proceed = match event {
        ModelEvent::Retrieved => observer.retrieved(model).await.map(|_| true)?,
        ModelEvent::Creating => observer.creating(model).await?,
        ModelEvent::Created => observer.created(model).await.map(|_| true)?,
        ModelEvent::Updating => observer.updating(model).await?,
        ModelEvent::Updated => observer.updated(model).await.map(|_| true)?,
        ModelEvent::Saving => observer.saving(model).await?,
        ModelEvent::Saved => observer.saved(model).await.map(|_| true)?,
        ModelEvent::Deleting => observer.deleting(model).await?,
        ModelEvent::Deleted => observer.deleted(model).await.map(|_| true)?,
        ModelEvent::Restoring => observer.restoring(model).await?,
        ModelEvent::Restored => observer.restored(model).await.map(|_| true)?,
        ModelEvent::ForceDeleting => observer.force_deleting(model).await?,
        ModelEvent::ForceDeleted => observer.force_deleted(model).await.map(|_| true)?,
        ModelEvent::Trashed => observer.trashed(model).await.map(|_| true)?,
        ModelEvent::Replicating => observer.replicating(model).await.map(|_| true)?,
    };
    Ok(proceed)
}

/// Fire a model event. Returns `false` when a listener cancelled a halting
/// event.
pub(crate) async fn fire<M: Model>(event: ModelEvent, model: &mut M) -> Result<bool> {
    if events_muted() {
        return Ok(true);
    }
    let state = EloquentState::resolve();
    state.boot::<M>();
    let listeners = state.listeners::<M>();
    drop(state);
    for listener in listeners {
        let proceed = match &listener {
            Listener::Closure(registered, callback) if *registered == event => callback(model)?,
            Listener::Closure(..) => true,
            Listener::Observer(observer) => dispatch(observer.as_ref(), event, model).await?,
        };
        if !proceed && event.is_halting() {
            return Ok(false);
        }
    }
    Ok(true)
}
