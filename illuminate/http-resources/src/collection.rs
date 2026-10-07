//! Resource collections: transform collections (and pages) of models.

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};

use illuminate_http::{Request, Response, request};
use illuminate_pagination::{LengthAwarePaginator, Paginator};
use illuminate_support::{Collection, Map, Value};

use crate::resource::{JsonResource, Resource};
use crate::response::{is_numeric, php_int_key};
use crate::state::ResourceState;

/// The resources of a collection (Laravel's `$this->collection`): every
/// model, mapped into the collected resource.
///
/// It serializes to the resolved resources, so a collection resource can
/// put it anywhere in its array:
///
/// ```ignore
/// json!({"data": self.collection(), "links": {"self": "link-value"}})
/// ```
#[derive(Clone, Debug)]
pub struct Resources<R> {
    items: Vec<R>,
    keys: Option<Vec<String>>,
    pub(crate) preserve_keys: bool,
}

impl<R: JsonResource> Resources<R> {
    /// Map the models into resources.
    pub fn from_models(models: impl IntoIterator<Item = R::Model>) -> Self {
        Self::new(models.into_iter().map(R::from_model).collect())
    }

    /// Map keyed models into resources, remembering their keys.
    pub fn from_keyed_models(models: impl IntoIterator<Item = (String, R::Model)>) -> Self {
        let (keys, items): (Vec<String>, Vec<R>) = models
            .into_iter()
            .map(|(key, model)| (key, R::from_model(model)))
            .unzip();
        Self {
            items,
            keys: Some(keys),
            preserve_keys: false,
        }
    }

    /// Resolve every resource, as a collection's default `to_array` does.
    ///
    /// Keyed collections become lists unless their keys are strings or the
    /// collection preserves its keys.
    pub fn resolve(&self, request: &Request) -> Value {
        let resolved = self.items.iter().map(|resource| resource.resolve(request));
        match &self.keys {
            Some(keys) if self.preserve_keys || !keys.iter().all(|key| is_numeric(key)) => {
                Value::Object(keys.iter().cloned().zip(resolved).collect::<Map<_, _>>())
            }
            _ => Value::Array(resolved.collect()),
        }
    }
}

impl<R> Resources<R> {
    /// A collection of resources.
    pub fn new(items: Vec<R>) -> Self {
        Self {
            items,
            keys: None,
            preserve_keys: false,
        }
    }

    /// The number of resources in the collection.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// The number of resources in the collection.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the collection is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The first resource.
    pub fn first(&self) -> Option<&R> {
        self.items.first()
    }

    /// Iterate over the resources.
    pub fn iter(&self) -> std::slice::Iter<'_, R> {
        self.items.iter()
    }

    /// The resources.
    pub fn all(&self) -> &[R] {
        &self.items
    }

    /// The collection's keys, if it was made from a keyed map.
    pub fn keys(&self) -> Option<&[String]> {
        self.keys.as_deref()
    }

    /// Whether the collection keeps its keys when resolved.
    pub fn preserves_keys(&self) -> bool {
        self.preserve_keys
    }

    /// Take the resources out.
    pub fn into_vec(self) -> Vec<R> {
        self.items
    }
}

impl<'a, R> IntoIterator for &'a Resources<R> {
    type Item = &'a R;
    type IntoIter = std::slice::Iter<'a, R>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<R> IntoIterator for Resources<R> {
    type Item = R;
    type IntoIter = std::vec::IntoIter<R>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

/// Serialized as-is, like a Laravel collection: a list, or an object when
/// the keys aren't `0..n`.
impl<R: JsonResource> Serialize for Resources<R> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let request = request();
        let sequential = self.keys.as_ref().is_none_or(|keys| {
            keys.iter()
                .enumerate()
                .all(|(index, key)| php_int_key(key) == Some(index as i64))
        });
        if sequential {
            let mut seq = serializer.serialize_seq(Some(self.items.len()))?;
            for resource in &self.items {
                seq.serialize_element(&resource.resolve(&request))?;
            }
            seq.end()
        } else {
            let keys = self.keys.as_deref().unwrap_or_default();
            let mut map = serializer.serialize_map(Some(self.items.len()))?;
            for (key, resource) in keys.iter().zip(&self.items) {
                map.serialize_entry(key, &resource.resolve(&request))?;
            }
            map.end()
        }
    }
}

/// A resource collection: transforms a collection of models, with its own
/// array, links and meta data.
///
/// ```
/// use illuminate_http::{IntoResponse, Request};
/// use illuminate_http_resources::{JsonResource, ResourceCollection, Resources};
/// use illuminate_support::{Value, json};
///
/// pub struct UserResource(pub Value);
///
/// impl JsonResource for UserResource {
///     type Model = Value;
///
///     fn from_model(user: Value) -> Self {
///         Self(user)
///     }
///
///     fn model(&self) -> &Value {
///         &self.0
///     }
/// }
///
/// pub struct UserCollection(pub Resources<UserResource>);
///
/// impl ResourceCollection for UserCollection {
///     type Collects = UserResource;
///
///     fn from_collection(collection: Resources<UserResource>) -> Self {
///         Self(collection)
///     }
///
///     fn collection(&self) -> &Resources<UserResource> {
///         &self.0
///     }
///
///     fn to_array(&self, _request: &Request) -> Value {
///         json!({"data": self.0, "links": {"self": "link-value"}})
///     }
/// }
///
/// let response = UserCollection::make(vec![json!({"id": 1})]).into_response();
///
/// assert_eq!(
///     response.json_body(),
///     json!({"data": [{"id": 1}], "links": {"self": "link-value"}})
/// );
/// ```
///
/// Every resource collection is a [`JsonResource`] too: make it with
/// `UserCollection::make(users)` (a vector, a collection or a paginator of
/// models), and add top-level data with `additional`.
pub trait ResourceCollection: Sized + Send + Sync + 'static {
    /// The resource each model is mapped into.
    type Collects: JsonResource;

    /// Wrap the mapped resources in the collection.
    fn from_collection(collection: Resources<Self::Collects>) -> Self;

    /// The mapped resources (Laravel's `$this->collection`).
    fn collection(&self) -> &Resources<Self::Collects>;

    /// Transform the resource collection into an array.
    ///
    /// By default, every resource is resolved.
    fn to_array(&self, request: &Request) -> Value {
        self.collection().resolve(request)
    }

    /// Get any additional data that should be returned with the collection
    /// array — only when it is the outermost resource of the response.
    fn with(&self, request: &Request) -> Value {
        let _ = request;
        Value::Object(Map::new())
    }

    /// Customize the outgoing response for the collection.
    fn with_response(&self, request: &Request, response: &mut Response) {
        let _ = (request, response);
    }

    /// Customize the `links` and `meta` information of a paginated
    /// response.
    ///
    /// ```ignore
    /// fn pagination_information(&self, _request: &Request, _paginated: &Value, mut default: Value) -> Value {
    ///     default["links"]["custom"] = json!("https://example.com");
    ///     default
    /// }
    /// ```
    fn pagination_information(
        &self,
        request: &Request,
        paginated: &Value,
        default: Value,
    ) -> Value {
        let _ = (request, paginated);
        default
    }

    /// The key the collection is wrapped in when it is the outermost
    /// resource.
    fn wrapper() -> Option<String> {
        ResourceState::resolve().wrapper_for::<Self>()
    }

    /// Whether the collection keeps its keys (Laravel's `#[PreserveKeys]`).
    fn preserve_keys() -> bool {
        false
    }
}

/// Every resource collection is a resource whose model is its collection.
impl<C: ResourceCollection> JsonResource for C {
    type Model = Resources<C::Collects>;

    fn from_model(mut collection: Resources<C::Collects>) -> Self {
        collection.preserve_keys = <C as ResourceCollection>::preserve_keys();
        C::from_collection(collection)
    }

    fn model(&self) -> &Resources<C::Collects> {
        self.collection()
    }

    fn to_array(&self, request: &Request) -> Value {
        ResourceCollection::to_array(self, request)
    }

    fn with(&self, request: &Request) -> Value {
        ResourceCollection::with(self, request)
    }

    fn with_response(&self, request: &Request, response: &mut Response) {
        ResourceCollection::with_response(self, request, response);
    }

    fn pagination_information(
        &self,
        request: &Request,
        paginated: &Value,
        default: Value,
    ) -> Value {
        ResourceCollection::pagination_information(self, request, paginated, default)
    }

    fn wrapper() -> Option<String> {
        <C as ResourceCollection>::wrapper()
    }

    fn preserve_keys() -> bool {
        <C as ResourceCollection>::preserve_keys()
    }
}

/// The collection behind [`AnonymousResourceCollection`]: `R::collection(..)`
/// without a dedicated collection type.
#[derive(Clone, Debug)]
pub struct AnonymousCollection<R>(Resources<R>);

impl<R: JsonResource> ResourceCollection for AnonymousCollection<R> {
    type Collects = R;

    fn from_collection(collection: Resources<R>) -> Self {
        Self(collection)
    }

    fn collection(&self) -> &Resources<R> {
        &self.0
    }

    fn preserve_keys() -> bool {
        R::preserve_keys()
    }
}

/// What `UserResource::collection(users)` returns: a collection of
/// `UserResource`s, ready to be returned from a route.
pub type AnonymousResourceCollection<R> = Resource<AnonymousCollection<R>>;

impl<R: JsonResource> Resource<AnonymousCollection<R>> {
    /// Keep the collection's keys (Laravel's `preserveKeys()`).
    ///
    /// ```ignore
    /// UserResource::collection(users.key_by(|user| user.id)).preserve_keys()
    /// ```
    pub fn preserve_keys(mut self) -> Self {
        if let crate::source::Slot::Present(collection) = &mut self.resource {
            collection.0.preserve_keys = true;
        }
        self
    }

    /// The mapped resources.
    pub fn collection(&self) -> Option<&Resources<R>> {
        self.resource().map(|collection| &collection.0)
    }

    /// The number of resources in the collection.
    pub fn count(&self) -> usize {
        self.collection().map_or(0, Resources::count)
    }
}

/// Collections and pages of models that know their default resource
/// (Laravel's `toResourceCollection()`).
///
/// ```ignore
/// Route::get("/users", || async { Ok::<_, Error>(User::all().await?.to_resource_collection()) });
/// ```
pub trait ToResourceCollection {
    /// The resource each model is mapped into.
    type Resource: JsonResource;

    /// Transform the models into an anonymous resource collection.
    fn to_resource_collection(self) -> AnonymousResourceCollection<Self::Resource>;
}

macro_rules! to_resource_collection {
    ($($ty:ident),*) => {$(
        impl<M: crate::resource::UseResource> ToResourceCollection for $ty<M> {
            type Resource = M::Resource;

            fn to_resource_collection(self) -> AnonymousResourceCollection<M::Resource> {
                M::Resource::collection(self)
            }
        }
    )*};
}

to_resource_collection!(Vec, Collection, LengthAwarePaginator, Paginator);
