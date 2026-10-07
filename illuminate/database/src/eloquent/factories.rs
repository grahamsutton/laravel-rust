//! Model factories: generate models with fake data for tests and seeders.
//!
//! ```ignore
//! #[derive(Default)]
//! pub struct UserFactory;
//!
//! impl Factory for UserFactory {
//!     type Model = User;
//!
//!     fn definition(&self, faker: &mut Faker) -> Value {
//!         json!({
//!             "name": faker.name(),
//!             "email": faker.unique(|f| f.safe_email()),
//!             "password": "password",
//!         })
//!     }
//! }
//!
//! #[derive(Debug, Clone, Default, Model)]
//! #[use_factory(UserFactory)]
//! pub struct User { /* ... */ }
//!
//! let users = User::factory()
//!     .count(3)
//!     .has(Post::factory().count(2), "posts")
//!     .create()
//!     .await?;
//! ```
//!
//! Named states are added with an extension trait:
//!
//! ```ignore
//! pub trait UserFactoryStates {
//!     fn unverified(self) -> Self;
//! }
//!
//! impl UserFactoryStates for FactoryBuilder<UserFactory> {
//!     fn unverified(self) -> Self {
//!         self.state(json!({"email_verified_at": null}))
//!     }
//! }
//! ```

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use illuminate_support::{Collection, Map, Result, Value};

use super::errors::RelationNotFoundException;
use super::faker::Faker;
use super::model::Model;
use super::relations::RelationKind;
use super::{Attributes, BoxFuture};

/// A model factory: defines the default attributes of a model.
pub trait Factory: Send + Sync + Default + 'static {
    /// The model the factory builds.
    type Model: Model;

    /// The model's default attributes, as a JSON object.
    fn definition(&self, faker: &mut Faker) -> Value;

    /// Configure every builder of this factory (register `after_making` /
    /// `after_creating` callbacks, default states, ...).
    fn configure(builder: FactoryBuilder<Self>) -> FactoryBuilder<Self> {
        builder
    }

    /// A new builder for the factory.
    fn new() -> FactoryBuilder<Self> {
        FactoryBuilder::new()
    }
}

/// Models with a factory (`#[use_factory(UserFactory)]`).
pub trait HasFactory: Model {
    /// The model's factory.
    type Factory: Factory<Model = Self>;

    /// A new factory builder for the model.
    fn factory() -> FactoryBuilder<Self::Factory> {
        FactoryBuilder::new()
    }
}

/// A sequence of states cycled through as models are made.
///
/// ```ignore
/// User::factory()
///     .count(4)
///     .sequence([json!({"admin": true}), json!({"admin": false})])
///     .create()
///     .await?;
/// ```
#[derive(Clone)]
pub struct Sequence {
    generator: Arc<dyn Fn(usize) -> Value + Send + Sync>,
}

impl fmt::Debug for Sequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sequence").finish_non_exhaustive()
    }
}

impl Sequence {
    /// Cycle through the given states.
    pub fn new(states: impl IntoIterator<Item = Value>) -> Self {
        let states: Vec<Value> = states.into_iter().collect();
        Self {
            generator: Arc::new(move |index| {
                if states.is_empty() {
                    Value::Object(Map::new())
                } else {
                    states[index % states.len()].clone()
                }
            }),
        }
    }

    /// Compute each state from the model's index.
    pub fn using(generator: impl Fn(usize) -> Value + Send + Sync + 'static) -> Self {
        Self {
            generator: Arc::new(generator),
        }
    }

    /// The state for the model at the given index.
    pub fn state(&self, index: usize) -> Value {
        (self.generator)(index)
    }
}

type StateFn = Arc<dyn Fn(&Map<String, Value>) -> Value + Send + Sync>;
type MakingCallback<M> = Arc<dyn Fn(&mut M) -> Result<()> + Send + Sync>;
type CreatingCallback<M> = Arc<dyn Fn(M) -> BoxFuture<'static, Result<()>> + Send + Sync>;

#[derive(Clone)]
enum State {
    Attributes(Map<String, Value>),
    Closure(StateFn),
    Sequence(Sequence),
}

#[derive(Clone)]
enum ParentSource {
    Factory(Arc<dyn ErasedFactory>),
    Attributes(Map<String, Value>),
}

/// A type-erased factory builder, for `has` and `for_` relationships.
pub(crate) trait ErasedFactory: Send + Sync {
    /// Create the models with extra attributes, returning their attributes.
    fn create_erased(
        &self,
        extra: Map<String, Value>,
    ) -> BoxFuture<'_, Result<Vec<Map<String, Value>>>>;
}

/// Builds and persists models from a [`Factory`].
pub struct FactoryBuilder<F: Factory> {
    factory: Arc<F>,
    count: Option<usize>,
    states: Vec<State>,
    has: Vec<(Arc<dyn ErasedFactory>, String)>,
    parents: Vec<(ParentSource, String)>,
    after_making: Vec<MakingCallback<F::Model>>,
    after_creating: Vec<CreatingCallback<F::Model>>,
    seed: Option<u64>,
}

impl<F: Factory> Clone for FactoryBuilder<F> {
    fn clone(&self) -> Self {
        Self {
            factory: self.factory.clone(),
            count: self.count,
            states: self.states.clone(),
            has: self.has.clone(),
            parents: self.parents.clone(),
            after_making: self.after_making.clone(),
            after_creating: self.after_creating.clone(),
            seed: self.seed,
        }
    }
}

impl<F: Factory> fmt::Debug for FactoryBuilder<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FactoryBuilder")
            .field("model", &F::Model::class_name())
            .field("count", &self.count)
            .field("states", &self.states.len())
            .finish_non_exhaustive()
    }
}

impl<F: Factory> Default for FactoryBuilder<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F: Factory> FactoryBuilder<F> {
    /// A new builder, configured by the factory.
    pub fn new() -> Self {
        F::configure(Self {
            factory: Arc::new(F::default()),
            count: None,
            states: Vec::new(),
            has: Vec::new(),
            parents: Vec::new(),
            after_making: Vec::new(),
            after_creating: Vec::new(),
            seed: None,
        })
    }

    /// The number of models to build.
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// Alias of [`count`](FactoryBuilder::count).
    pub fn times(self, count: usize) -> Self {
        self.count(count)
    }

    /// Seed the fake data generator, for reproducible data.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Override attributes: `state(json!({"admin": true}))`.
    pub fn state(mut self, state: impl Into<Attributes>) -> Self {
        self.states.push(State::Attributes(state.into().0));
        self
    }

    /// Override attributes with a closure receiving the attributes so far.
    pub fn state_fn(
        mut self,
        state: impl Fn(&Map<String, Value>) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.states.push(State::Closure(Arc::new(state)));
        self
    }

    /// Cycle through states as models are made.
    pub fn sequence(mut self, states: impl IntoIterator<Item = Value>) -> Self {
        self.states.push(State::Sequence(Sequence::new(states)));
        self
    }

    /// Apply a [`Sequence`].
    pub fn sequence_with(mut self, sequence: Sequence) -> Self {
        self.states.push(State::Sequence(sequence));
        self
    }

    /// Compute a state from each model's index.
    pub fn sequence_fn(self, generator: impl Fn(usize) -> Value + Send + Sync + 'static) -> Self {
        self.sequence_with(Sequence::using(generator))
    }

    /// Create related models for each model through a has-one, has-many,
    /// morph or many-to-many relationship.
    ///
    /// ```ignore
    /// User::factory().has(Post::factory().count(3), "posts").create().await?;
    /// ```
    pub fn has<G: Factory>(mut self, factory: FactoryBuilder<G>, relation: &str) -> Self {
        self.has.push((Arc::new(factory), relation.to_string()));
        self
    }

    /// Create a parent model for a belongs-to relationship (shared by every
    /// model built).
    ///
    /// ```ignore
    /// Post::factory().count(3).for_(User::factory(), "user").create().await?;
    /// ```
    pub fn for_<G: Factory>(mut self, factory: FactoryBuilder<G>, relation: &str) -> Self {
        self.parents.push((
            ParentSource::Factory(Arc::new(factory)),
            relation.to_string(),
        ));
        self
    }

    /// Use an existing model as the parent of a belongs-to relationship.
    pub fn for_model<P: Model>(mut self, parent: &P, relation: &str) -> Self {
        self.parents.push((
            ParentSource::Attributes(parent.to_attributes()),
            relation.to_string(),
        ));
        self
    }

    /// Run a callback on each model after it is made.
    pub fn after_making(
        mut self,
        callback: impl Fn(&mut F::Model) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.after_making.push(Arc::new(callback));
        self
    }

    /// Run an async callback with each model after it is created.
    ///
    /// ```ignore
    /// User::factory()
    ///     .after_creating(|user: User| async move {
    ///         Profile::create(json!({"user_id": user.id})).await?;
    ///         Ok(())
    ///     })
    ///     .create()
    ///     .await?;
    /// ```
    pub fn after_creating<Fut>(
        mut self,
        callback: impl Fn(F::Model) -> Fut + Send + Sync + 'static,
    ) -> Self
    where
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.after_creating.push(Arc::new(move |model| {
            Box::pin(callback(model)) as BoxFuture<'static, Result<()>>
        }));
        self
    }

    fn faker(&self) -> Faker {
        match self.seed {
            Some(seed) => Faker::seeded(seed),
            None => Faker::new(),
        }
    }

    /// Create the parents of belongs-to relationships, returning the
    /// foreign key attributes they contribute.
    async fn resolve_parents(&self) -> Result<Map<String, Value>> {
        let mut attributes = Map::new();
        for (source, name) in &self.parents {
            let relation = F::Model::relation(name).ok_or_else(|| {
                RelationNotFoundException::new(F::Model::class_name(), name.as_str())
            })?;
            let parent = match source {
                ParentSource::Attributes(parent) => parent.clone(),
                ParentSource::Factory(factory) => factory
                    .create_erased(Map::new())
                    .await?
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
            };
            for (key, value) in relation.attributes_for_parent(&parent) {
                attributes.insert(key, value);
            }
        }
        Ok(attributes)
    }

    fn build_attributes(
        &self,
        faker: &mut Faker,
        parents: &Map<String, Value>,
    ) -> Vec<Map<String, Value>> {
        let count = self.count.unwrap_or(1);
        (0..count)
            .map(|index| {
                let mut attributes = match self.factory.definition(faker) {
                    Value::Object(map) => map,
                    _ => Map::new(),
                };
                for (key, value) in parents {
                    attributes.insert(key.clone(), value.clone());
                }
                for state in &self.states {
                    let overrides = match state {
                        State::Attributes(map) => Value::Object(map.clone()),
                        State::Closure(closure) => closure(&attributes),
                        State::Sequence(sequence) => sequence.state(index),
                    };
                    if let Value::Object(overrides) = overrides {
                        for (key, value) in overrides {
                            attributes.insert(key, value);
                        }
                    }
                }
                attributes
            })
            .collect()
    }

    /// The raw attributes the models would be made with.
    pub async fn raw(&self) -> Result<Vec<Map<String, Value>>> {
        let parents = self.resolve_parents().await?;
        let mut faker = self.faker();
        Ok(self.build_attributes(&mut faker, &parents))
    }

    /// Make the models without saving them.
    pub async fn make(&self) -> Result<Collection<F::Model>> {
        let parents = self.resolve_parents().await?;
        let mut faker = self.faker();
        let mut models = Vec::new();
        for attributes in self.build_attributes(&mut faker, &parents) {
            let mut model = F::Model::template();
            model.force_fill(attributes)?;
            for callback in &self.after_making {
                callback(&mut model)?;
            }
            models.push(model);
        }
        Ok(models.into())
    }

    /// Make a single model without saving it.
    pub async fn make_one(&self) -> Result<F::Model> {
        let models = self.clone().count(1).make().await?;
        Ok(models.into_vec().remove(0))
    }

    /// Make and save the models (and their `has` relationships).
    pub async fn create(&self) -> Result<Collection<F::Model>> {
        let mut models = self.make().await?.into_vec();
        for model in models.iter_mut() {
            model.save().await?;
            let parent = model.to_attributes();
            for (factory, name) in &self.has {
                create_children::<F::Model>(factory.as_ref(), name, &parent).await?;
            }
        }
        for model in &models {
            for callback in &self.after_creating {
                callback(model.clone()).await?;
            }
        }
        Ok(models.into())
    }

    /// Make and save a single model.
    pub async fn create_one(&self) -> Result<F::Model> {
        let models = self.clone().count(1).create().await?;
        Ok(models.into_vec().remove(0))
    }
}

async fn create_children<M: Model>(
    factory: &dyn ErasedFactory,
    name: &str,
    parent: &Map<String, Value>,
) -> Result<()> {
    let relation =
        M::relation(name).ok_or_else(|| RelationNotFoundException::new(M::class_name(), name))?;
    match relation.kind() {
        RelationKind::BelongsToMany => {
            let created = factory.create_erased(Map::new()).await?;
            relation.attach_models(parent, &created).await
        }
        RelationKind::HasOne | RelationKind::HasMany => {
            factory
                .create_erased(relation.attributes_for_related(parent))
                .await?;
            Ok(())
        }
        kind => Err(anyhow::anyhow!(
            "Factories can't create related models through a [{kind:?}] relationship ([{name}] on [{}]).",
            M::class_name()
        )),
    }
}

impl<F: Factory> ErasedFactory for FactoryBuilder<F> {
    fn create_erased(
        &self,
        extra: Map<String, Value>,
    ) -> BoxFuture<'_, Result<Vec<Map<String, Value>>>> {
        Box::pin(async move {
            let models = self.clone().state(extra).create().await?;
            Ok(models.iter().map(|model| model.to_attributes()).collect())
        })
    }
}
