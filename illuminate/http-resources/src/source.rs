//! What resources are made from: models (owned, borrowed or boxed),
//! `Option`s, potentially missing values, collections, keyed maps and
//! paginators.

use std::fmt::Display;

use indexmap::IndexMap;

use illuminate_pagination::{LengthAwarePaginator, Paginator};
use illuminate_support::{Collection, Map, Value};

use crate::collection::Resources;
use crate::conditional::PotentiallyMissing;
use crate::resource::JsonResource;

/// The state of a resource's underlying value.
#[derive(Clone, Debug)]
pub(crate) enum Slot<M> {
    /// A model (or collection) is present.
    Present(M),
    /// The resource wraps `null`.
    Null,
    /// The resource wraps a missing value: it is left out of the JSON.
    Missing,
}

impl<M> Slot<M> {
    pub(crate) fn map<U>(self, callback: impl FnOnce(M) -> U) -> Slot<U> {
        match self {
            Self::Present(model) => Slot::Present(callback(model)),
            Self::Null => Slot::Null,
            Self::Missing => Slot::Missing,
        }
    }
}

/// The paginator behind a paginated resource collection.
///
/// It keeps everything a paginator knows except the items, which have
/// already been mapped into resources.
#[derive(Clone, Debug)]
pub enum Pagination {
    /// A length-aware paginator (`paginate()`).
    LengthAware(LengthAwarePaginator<()>),
    /// A simple paginator (`simple_paginate()`).
    Simple(Paginator<()>),
}

impl Pagination {
    /// The paginator's array form (Laravel's `$paginated`), without its
    /// `data`.
    pub fn to_array(&self) -> Value {
        let mut array = match self {
            Self::LengthAware(paginator) => paginator.to_array(),
            Self::Simple(paginator) => paginator.to_array(),
        };
        if let Value::Object(map) = &mut array {
            map.shift_remove("data");
        }
        array
    }

    /// Append query string values to every page URL.
    pub fn appends(self, query: Map<String, Value>) -> Self {
        match self {
            Self::LengthAware(paginator) => Self::LengthAware(paginator.appends(query)),
            Self::Simple(paginator) => Self::Simple(paginator.appends(query)),
        }
    }
}

/// A resource's underlying value, ready to be wrapped: a model, `null`, or a
/// missing value — and the paginator, when there is one.
///
/// Implement [`IntoResource`] with these constructors to make resources
/// from your own types.
#[derive(Clone, Debug)]
pub struct Source<M> {
    pub(crate) value: Slot<M>,
    pub(crate) pagination: Option<Pagination>,
}

impl<M> Source<M> {
    /// The resource wraps the given model.
    pub fn model(model: M) -> Self {
        Self {
            value: Slot::Present(model),
            pagination: None,
        }
    }

    /// The resource wraps `null`: nested, it renders as `null`.
    pub fn null() -> Self {
        Self {
            value: Slot::Null,
            pagination: None,
        }
    }

    /// The resource wraps a missing value: nested, its key is removed.
    pub fn missing() -> Self {
        Self {
            value: Slot::Missing,
            pagination: None,
        }
    }

    /// The resource wraps a page of models.
    pub fn paginated(model: M, pagination: Pagination) -> Self {
        Self {
            value: Slot::Present(model),
            pagination: Some(pagination),
        }
    }

    fn optional(model: Option<M>) -> Self {
        model.map(Self::model).unwrap_or_else(Self::null)
    }

    fn potentially_missing(model: PotentiallyMissing<M>) -> Self {
        match model {
            PotentiallyMissing::Present(model) => Self::model(model),
            PotentiallyMissing::Missing => Self::missing(),
        }
    }

    /// Whether the source is missing.
    pub fn is_missing(&self) -> bool {
        matches!(self.value, Slot::Missing)
    }

    /// Whether the source is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self.value, Slot::Null)
    }

    /// The paginator, if the source is a page of models.
    pub fn pagination(&self) -> Option<&Pagination> {
        self.pagination.as_ref()
    }
}

/// Things a resource can be made from.
///
/// A resource is made from its model — owned, borrowed (it is cloned) or
/// boxed — or from an `Option` (`None` renders as `null`) or a
/// [`PotentiallyMissing`] value (missing values are left out). Collections
/// are made from vectors, slices, [`Collection`]s, keyed `IndexMap`s and
/// paginators of models.
pub trait IntoResource<M> {
    /// Convert into the resource's source.
    fn into_resource(self) -> Source<M>;
}

macro_rules! model_sources {
    ($( [$($generics:tt)*] $ty:ty => |$source:ident| $body:expr; )*) => {$(
        impl<$($generics)*> IntoResource<M> for $ty {
            fn into_resource(self) -> Source<M> {
                let $source = self;
                $body
            }
        }
    )*};
}

model_sources! {
    [M] M => |model| Source::model(model);
    ['a, M: Clone] &'a M => |model| Source::model(model.clone());
    [M] Box<M> => |model| Source::model(*model);
    ['a, M: Clone] &'a Box<M> => |model| Source::model((**model).clone());
    [M] Option<M> => |model| Source::optional(model);
    [M] Option<Box<M>> => |model| Source::optional(model.map(|model| *model));
    ['a, M: Clone] Option<&'a M> => |model| Source::optional(model.cloned());
    ['a, M: Clone] Option<&'a Box<M>> => |model| Source::optional(model.map(|model| (**model).clone()));
    ['a, M: Clone] &'a Option<M> => |model| Source::optional(model.clone());
    ['a, M: Clone] &'a Option<Box<M>> => |model| Source::optional(model.as_deref().cloned());
    [M] PotentiallyMissing<M> => |model| Source::potentially_missing(model);
    [M] PotentiallyMissing<Box<M>> => |model| Source::potentially_missing(model.map(|model| *model));
    ['a, M: Clone] PotentiallyMissing<&'a M> => |model| Source::potentially_missing(model.map(Clone::clone));
    ['a, M: Clone] PotentiallyMissing<&'a Box<M>> => |model| Source::potentially_missing(model.map(|model| (**model).clone()));
}

macro_rules! collection_sources {
    ($( [$($generics:tt)*] [$($bounds:tt)*] $ty:ty => |$source:ident| $body:expr; )*) => {$(
        impl<$($generics)* R: JsonResource> IntoResource<Resources<R>> for $ty
        where
            $($bounds)*
        {
            fn into_resource(self) -> Source<Resources<R>> {
                let $source = self;
                $body
            }
        }
    )*};
}

fn list<R: JsonResource>(models: impl IntoIterator<Item = R::Model>) -> Source<Resources<R>> {
    Source::model(Resources::from_models(models))
}

fn optional_list<R: JsonResource>(
    models: Option<impl IntoIterator<Item = R::Model>>,
) -> Source<Resources<R>> {
    models.map(list).unwrap_or_else(Source::null)
}

fn missing_list<R: JsonResource>(
    models: PotentiallyMissing<impl IntoIterator<Item = R::Model>>,
) -> Source<Resources<R>> {
    match models {
        PotentiallyMissing::Present(models) => list(models),
        PotentiallyMissing::Missing => Source::missing(),
    }
}

collection_sources! {
    [] [] Vec<R::Model> => |models| list(models);
    [] [] Collection<R::Model> => |models| list(models);
    ['a,] [R::Model: Clone] &'a Vec<R::Model> => |models| list(models.iter().cloned());
    ['a,] [R::Model: Clone] &'a [R::Model] => |models| list(models.iter().cloned());
    ['a,] [R::Model: Clone] &'a Collection<R::Model> => |models| list(models.iter().cloned());
    [] [] Option<Vec<R::Model>> => |models| optional_list(models);
    [] [] Option<Collection<R::Model>> => |models| optional_list(models);
    ['a,] [R::Model: Clone] Option<&'a Vec<R::Model>> => |models| optional_list(models.map(|m| m.iter().cloned()));
    ['a,] [R::Model: Clone] Option<&'a Collection<R::Model>> => |models| optional_list(models.map(|m| m.iter().cloned()));
    ['a,] [R::Model: Clone] &'a Option<Vec<R::Model>> => |models| optional_list(models.as_ref().map(|m| m.iter().cloned()));
    ['a,] [R::Model: Clone] &'a Option<Collection<R::Model>> => |models| optional_list(models.as_ref().map(|m| m.iter().cloned()));
    [] [] PotentiallyMissing<Vec<R::Model>> => |models| missing_list(models);
    [] [] PotentiallyMissing<Collection<R::Model>> => |models| missing_list(models);
    ['a,] [R::Model: Clone] PotentiallyMissing<&'a Vec<R::Model>> => |models| missing_list(models.map(|m| m.iter().cloned()));
    ['a,] [R::Model: Clone] PotentiallyMissing<&'a Collection<R::Model>> => |models| missing_list(models.map(|m| m.iter().cloned()));
    [K: Display,] [] IndexMap<K, R::Model> => |models| Source::model(Resources::from_keyed_models(
        models.into_iter().map(|(key, model)| (key.to_string(), model)),
    ));
    [] [] LengthAwarePaginator<R::Model> => |paginator| {
        let mut models = Vec::with_capacity(paginator.count());
        let pagination = paginator.through(|model| models.push(model));
        Source::paginated(Resources::from_models(models), Pagination::LengthAware(pagination))
    };
    [] [] Paginator<R::Model> => |paginator| {
        let mut models = Vec::with_capacity(paginator.count());
        let pagination = paginator.through(|model| models.push(model));
        Source::paginated(Resources::from_models(models), Pagination::Simple(pagination))
    };
}
