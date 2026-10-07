//! `JsonResource`: the transformation layer between your models and the
//! JSON your API returns.

use serde::{Serialize, Serializer};

use illuminate_database::eloquent::Model as Eloquent;
use illuminate_http::{IntoResponse, Request, Response, request, with_request_sync};
use illuminate_support::{Map, Str, Value, to_value};

use crate::collection::{AnonymousResourceCollection, Resources};
use crate::conditional::{
    Loadable, MergeValue, MissingValue, Nullable, PotentiallyMissing, filter, is_missing,
};
use crate::response::{array_merge_recursive, pagination_information, wrap};
use crate::source::{IntoResource, Pagination, Slot, Source};
use crate::state::ResourceState;

/// A resource: transforms a model into the JSON your API returns.
///
/// A resource wraps a single model. Implement `to_array` to decide which of
/// its attributes make it into the response, and use the conditional
/// helpers (`when`, `when_loaded`, `when_counted`, `merge_when`, ...) to
/// include attributes only when it makes sense:
///
/// ```
/// use illuminate_http::Request;
/// use illuminate_http_resources::JsonResource;
/// use illuminate_support::{Value, json};
///
/// #[derive(Clone, serde::Serialize)]
/// pub struct User {
///     pub id: u64,
///     pub name: String,
///     pub email: String,
/// }
///
/// pub struct UserResource(pub User);
///
/// impl JsonResource for UserResource {
///     type Model = User;
///
///     fn from_model(user: User) -> Self {
///         Self(user)
///     }
///
///     fn model(&self) -> &User {
///         &self.0
///     }
///
///     fn to_array(&self, request: &Request) -> Value {
///         json!({
///             "id": self.0.id,
///             "name": self.0.name,
///             "email": self.0.email,
///             "secret": self.when(request.boolean("admin"), || "secret-value"),
///         })
///     }
/// }
///
/// let user = User { id: 1, name: "Taylor".into(), email: "taylor@laravel.com".into() };
///
/// let response = UserResource::make(user).response();
///
/// assert_eq!(
///     response.json_body(),
///     json!({"data": {"id": 1, "name": "Taylor", "email": "taylor@laravel.com"}})
/// );
/// ```
///
/// Return a resource straight from a route — `UserResource::make(user)`,
/// `UserResource::collection(users)` or a paginator — and it becomes a JSON
/// response. Without a `to_array` of your own, the model is serialized as
/// is (respecting its hidden and visible attributes), like Laravel's
/// `parent::toArray($request)`.
///
/// ## Models carry no hidden state
///
/// Laravel answers `201 Created` for models that were just created. Rust
/// models don't remember that, so say it yourself:
/// `(201, UserResource::make(user))` or
/// `UserResource::make(user).response().with_status(201)`.
pub trait JsonResource: Sized + Send + Sync + 'static {
    /// The model the resource transforms (an Eloquent model, or anything
    /// serializable).
    type Model: Serialize + Send + Sync + 'static;

    /// Wrap a model in the resource.
    fn from_model(model: Self::Model) -> Self;

    /// The model wrapped by the resource (Laravel's `$this->resource`).
    fn model(&self) -> &Self::Model;

    /// Transform the resource into an array.
    ///
    /// By default, the model is serialized as is.
    fn to_array(&self, request: &Request) -> Value {
        let _ = request;
        to_value(self.model())
    }

    /// Get any additional data that should be returned with the resource
    /// array — only when it is the outermost resource of the response.
    fn with(&self, request: &Request) -> Value {
        let _ = request;
        Value::Object(Map::new())
    }

    /// Customize the outgoing response for the resource (only called when
    /// it is the outermost resource of the response).
    fn with_response(&self, request: &Request, response: &mut Response) {
        let _ = (request, response);
    }

    /// Customize the `links` and `meta` information of a paginated
    /// response. `paginated` is the paginator's array form and `default`
    /// holds the `links` and `meta` keys Laravel would return.
    fn pagination_information(
        &self,
        request: &Request,
        paginated: &Value,
        default: Value,
    ) -> Value {
        let _ = (request, paginated);
        default
    }

    /// The key the outermost resource is wrapped in (`data`).
    ///
    /// Change it for every resource with [`Resource::wrap`], for this
    /// resource with [`JsonResource::wrap`], or override it here.
    fn wrapper() -> Option<String> {
        ResourceState::resolve().wrapper_for::<Self>()
    }

    /// Whether collections of this resource keep their keys (Laravel's
    /// `#[PreserveKeys]`).
    fn preserve_keys() -> bool {
        false
    }

    // ------------------------------------------------------------------
    // Making resources
    // ------------------------------------------------------------------

    /// Create a new resource instance.
    ///
    /// Accepts the model itself, a reference to it (it is cloned), a box,
    /// an `Option` (`None` renders as `null`) or a [`PotentiallyMissing`]
    /// model (a missing model is left out of its parent's JSON).
    fn make(model: impl IntoResource<Self::Model>) -> Resource<Self> {
        Resource::from_source(model.into_resource())
    }

    /// Create a new resource instance (an alias of [`make`](Self::make)).
    fn new(model: impl IntoResource<Self::Model>) -> Resource<Self> {
        Self::make(model)
    }

    /// Create a new anonymous resource collection.
    ///
    /// Accepts vectors, slices and collections of models, keyed `IndexMap`s,
    /// paginators, and `Option` or [`PotentiallyMissing`] collections (a
    /// relationship field passed through [`when_loaded`](Self::when_loaded)).
    fn collection(models: impl IntoResource<Resources<Self>>) -> AnonymousResourceCollection<Self> {
        AnonymousResourceCollection::<Self>::from_source(models.into_resource())
    }

    /// Resolve the resource to an array: [`to_array`](Self::to_array) with
    /// its missing values removed and its merged values merged.
    fn resolve(&self, request: &Request) -> Value {
        with_request_sync(request.clone(), || {
            let data = self.to_array(request);
            if is_missing(&data) {
                return Value::Array(Vec::new());
            }
            filter(data)
        })
    }

    // ------------------------------------------------------------------
    // Wrapping
    // ------------------------------------------------------------------

    /// Wrap this resource in the given key when it is the outermost
    /// resource (Laravel's `public static $wrap = 'user'`).
    fn wrap(key: impl Into<String>) {
        ResourceState::resolve().set_wrapper_for::<Self>(Some(key.into()));
    }

    /// Disable wrapping of this resource.
    fn without_wrapping() {
        ResourceState::resolve().set_wrapper_for::<Self>(None);
    }

    // ------------------------------------------------------------------
    // Conditional attributes
    // ------------------------------------------------------------------

    /// Retrieve a value if the given condition is true.
    ///
    /// The value is given as a closure, so it is only computed when it is
    /// needed. Chain `unwrap_or(default)` for Laravel's third argument.
    ///
    /// ```ignore
    /// "secret": self.when(request.user().is_admin(), || "secret-value"),
    /// "role": self.when(user.is_admin, || "admin").unwrap_or("member"),
    /// ```
    fn when<T>(&self, condition: bool, value: impl FnOnce() -> T) -> PotentiallyMissing<T> {
        if condition {
            PotentiallyMissing::Present(value())
        } else {
            PotentiallyMissing::Missing
        }
    }

    /// Retrieve a value unless the given condition is true.
    fn unless<T>(&self, condition: bool, value: impl FnOnce() -> T) -> PotentiallyMissing<T> {
        self.when(!condition, value)
    }

    /// Retrieve a value if it is not null.
    ///
    /// ```ignore
    /// "nickname": self.when_not_null(&self.0.nickname),
    /// ```
    fn when_not_null<N: Nullable>(&self, value: N) -> PotentiallyMissing<N::Item> {
        value.into_option().into()
    }

    /// Include `null` if the value is null; leave the key out otherwise.
    fn when_null<N: Nullable>(&self, value: N) -> PotentiallyMissing<Value> {
        match value.into_option() {
            None => PotentiallyMissing::Present(Value::Null),
            Some(_) => PotentiallyMissing::Missing,
        }
    }

    /// Merge a value into the array.
    fn merge(&self, value: impl Serialize) -> MergeValue {
        MergeValue::new(value)
    }

    /// Merge a value into the array if the given condition is true.
    ///
    /// ```ignore
    /// "admin": self.merge_when(request.user().is_admin(), || json!({
    ///     "first-secret": "value",
    ///     "second-secret": "value",
    /// })),
    /// ```
    fn merge_when<V: Serialize>(
        &self,
        condition: bool,
        value: impl FnOnce() -> V,
    ) -> PotentiallyMissing<MergeValue> {
        if condition {
            PotentiallyMissing::Present(MergeValue::new(value()))
        } else {
            PotentiallyMissing::Missing
        }
    }

    /// Merge a value into the array unless the given condition is true.
    fn merge_unless<V: Serialize>(
        &self,
        condition: bool,
        value: impl FnOnce() -> V,
    ) -> PotentiallyMissing<MergeValue> {
        self.merge_when(!condition, value)
    }

    /// Merge the given attributes of the model into the array.
    ///
    /// ```ignore
    /// "attributes": self.attributes(&["id", "name", "email"]),
    /// ```
    fn attributes(&self, attributes: &[&str]) -> MergeValue {
        let data: Map<String, Value> = match to_value(self.model()) {
            Value::Object(map) => map
                .into_iter()
                .filter(|(key, _)| attributes.contains(&key.as_str()))
                .collect(),
            _ => Map::new(),
        };
        MergeValue::new(data)
    }

    /// Retrieve an attribute if it is present on the model.
    ///
    /// Persisted columns are always present; `#[computed]` attributes (and
    /// appended accessors) are present when they hold a value.
    fn when_has(&self, attribute: &str) -> PotentiallyMissing<Value>
    where
        Self::Model: Eloquent,
    {
        let model = self.model();
        let column = <Self::Model as Eloquent>::columns().contains(&attribute)
            || <Self::Model as Eloquent>::appends().contains(&attribute);
        match attribute_value(model, attribute) {
            Some(value) => PotentiallyMissing::Present(value),
            None if column => PotentiallyMissing::Present(Value::Null),
            None => PotentiallyMissing::Missing,
        }
    }

    /// Retrieve an accessor when it has been appended (`#[appends(...)]`).
    fn when_appended(&self, attribute: &str) -> PotentiallyMissing<Value>
    where
        Self::Model: Eloquent,
    {
        if !<Self::Model as Eloquent>::appends().contains(&attribute) {
            return PotentiallyMissing::Missing;
        }
        PotentiallyMissing::Present(attribute_value(self.model(), attribute).unwrap_or(Value::Null))
    }

    /// Retrieve a relationship if it has been loaded.
    ///
    /// Pass the relationship's field for the loaded models themselves, or
    /// its name for their serialized form:
    ///
    /// ```ignore
    /// "posts": PostResource::collection(self.when_loaded(&self.0.posts)),
    /// "author": self.when_loaded(&self.0.author).map(UserResource::make),
    /// "comments": self.when_loaded("comments"),
    /// ```
    fn when_loaded<'a, L: Loadable<'a, Self::Model>>(
        &'a self,
        relationship: L,
    ) -> PotentiallyMissing<L::Output> {
        relationship.loaded(self.model()).into()
    }

    /// Retrieve a relationship count if it has been loaded
    /// (`with_count("posts")` → `posts_count`).
    fn when_counted(&self, relationship: &str) -> PotentiallyMissing<Value>
    where
        Self::Model: Eloquent,
    {
        let attribute = Str::finish(&Str::snake(relationship), "_count");
        attribute_value(self.model(), &attribute).into()
    }

    /// Retrieve a relationship aggregate if it has been loaded
    /// (`with_sum("posts", "words")` → `posts_sum_words`).
    ///
    /// Aggregates of empty relationships are `null`, which is
    /// indistinguishable from "not loaded" on a plain struct: they are left
    /// out.
    fn when_aggregated(
        &self,
        relationship: &str,
        column: &str,
        aggregate: &str,
    ) -> PotentiallyMissing<Value>
    where
        Self::Model: Eloquent,
    {
        let attribute = Str::finish(
            &format!("{}_{aggregate}_", Str::snake(relationship)),
            column,
        );
        attribute_value(self.model(), &attribute).into()
    }

    /// Retrieve a relationship existence check if it has been loaded
    /// (`with_exists("posts")` → `posts_exists`).
    fn when_exists_loaded(&self, relationship: &str) -> PotentiallyMissing<Value>
    where
        Self::Model: Eloquent,
    {
        let attribute = Str::finish(&Str::snake(relationship), "_exists");
        attribute_value(self.model(), &attribute).into()
    }

    /// Retrieve a value if the model was loaded through the given pivot
    /// table. The callback receives the pivot attributes.
    ///
    /// ```ignore
    /// "expires_at": self.when_pivot_loaded("role_user", |pivot| pivot["expires_at"].clone()),
    /// ```
    fn when_pivot_loaded<T>(
        &self,
        table: &str,
        value: impl FnOnce(Value) -> T,
    ) -> PotentiallyMissing<T>
    where
        Self::Model: Eloquent,
    {
        self.when_pivot_loaded_as("pivot", table, value)
    }

    /// Retrieve a value if the model was loaded through the given pivot
    /// table, with a custom pivot accessor.
    fn when_pivot_loaded_as<T>(
        &self,
        accessor: &str,
        table: &str,
        value: impl FnOnce(Value) -> T,
    ) -> PotentiallyMissing<T>
    where
        Self::Model: Eloquent,
    {
        if !self.has_pivot_loaded_as(accessor, table) {
            return PotentiallyMissing::Missing;
        }
        PotentiallyMissing::Present(value(self.model().get_attribute(accessor)))
    }

    /// Determine if the model was loaded through the given pivot table.
    fn has_pivot_loaded(&self, table: &str) -> bool
    where
        Self::Model: Eloquent,
    {
        self.has_pivot_loaded_as("pivot", table)
    }

    /// Determine if the model was loaded through the given pivot table,
    /// with a custom pivot accessor.
    ///
    /// Pivot attributes don't record the table they came from, so this
    /// checks that the accessor holds pivot attributes.
    fn has_pivot_loaded_as(&self, accessor: &str, table: &str) -> bool
    where
        Self::Model: Eloquent,
    {
        let _ = table;
        matches!(self.model().get_attribute(accessor), Value::Object(pivot) if !pivot.is_empty())
    }
}

/// An attribute's serialized value, if the model has one.
fn attribute_value<M: Eloquent>(model: &M, attribute: &str) -> Option<Value> {
    let value = model
        .to_array()
        .get(attribute)
        .cloned()
        .unwrap_or_else(|| model.get_attribute(attribute));
    (!value.is_null()).then_some(value)
}

/// A resource instance, ready to be returned from a route or nested in
/// another resource.
///
/// This is what [`JsonResource::make`] (and `collection`) return. Return it
/// from a route and it becomes a JSON response; put it in another
/// resource's array and it is resolved in place:
///
/// ```
/// use illuminate_http::{IntoResponse, Request};
/// use illuminate_http_resources::{JsonResource, Resource};
/// use illuminate_support::{Value, json};
///
/// struct TagResource(Value);
///
/// impl JsonResource for TagResource {
///     type Model = Value;
///
///     fn from_model(tag: Value) -> Self {
///         Self(tag)
///     }
///
///     fn model(&self) -> &Value {
///         &self.0
///     }
/// }
///
/// let response = TagResource::make(json!({"name": "laravel"}))
///     .additional(json!({"meta": {"version": 1}}))
///     .into_response();
///
/// assert_eq!(
///     response.json_body(),
///     json!({"data": {"name": "laravel"}, "meta": {"version": 1}})
/// );
/// assert_eq!(response.header("content-type").unwrap(), "application/json");
/// ```
///
/// `Resource` is also where the global wrapping settings live — Laravel's
/// `JsonResource::withoutWrapping()` is `Resource::without_wrapping()`.
#[derive(Debug)]
pub struct Resource<T> {
    pub(crate) resource: Slot<T>,
    pub(crate) additional: Value,
    pub(crate) pagination: Option<Pagination>,
    pub(crate) query: QueryParameters,
}

/// The query string parameters added to pagination links.
#[derive(Clone, Debug, Default)]
pub(crate) enum QueryParameters {
    /// Leave the links alone.
    #[default]
    None,
    /// Append the request's query string.
    Preserve,
    /// Append the given parameters.
    Only(Map<String, Value>),
}

impl<T: Clone> Clone for Resource<T> {
    fn clone(&self) -> Self {
        Self {
            resource: self.resource.clone(),
            additional: self.additional.clone(),
            pagination: self.pagination.clone(),
            query: self.query.clone(),
        }
    }
}

impl Resource<()> {
    /// Disable wrapping of the outermost resource, for every resource
    /// (Laravel's `JsonResource::withoutWrapping()`).
    ///
    /// Paginated responses are still wrapped, since they carry `links` and
    /// `meta` keys.
    ///
    /// ```
    /// use illuminate_http_resources::Resource;
    ///
    /// Resource::without_wrapping();
    /// assert_eq!(Resource::wrapper(), None);
    ///
    /// Resource::flush_state();
    /// assert_eq!(Resource::wrapper().as_deref(), Some("data"));
    /// ```
    pub fn without_wrapping() {
        ResourceState::resolve().set_global_wrapper(None);
    }

    /// Set the key every resource is wrapped in (Laravel's
    /// `JsonResource::wrap('payload')`).
    pub fn wrap(key: impl Into<String>) {
        ResourceState::resolve().set_global_wrapper(Some(key.into()));
    }

    /// The key resources are wrapped in, unless they set their own.
    pub fn wrapper() -> Option<String> {
        ResourceState::resolve().global_wrapper()
    }

    /// Wrap the outermost resource even when its array already holds the
    /// wrapper key (Laravel's `JsonResource::$forceWrapping`).
    pub fn force_wrapping(force: bool) {
        ResourceState::resolve().set_force_wrapping(force);
    }

    /// Reset the wrapping settings to their defaults.
    pub fn flush_state() {
        ResourceState::resolve().flush();
    }
}

impl<T: JsonResource> Resource<T> {
    /// Create a resource instance from its source.
    pub(crate) fn from_source(source: Source<T::Model>) -> Self {
        Self {
            resource: source.value.map(T::from_model),
            additional: Value::Object(Map::new()),
            pagination: source.pagination,
            query: QueryParameters::None,
        }
    }

    /// The underlying resource (`None` when it wraps `null` or a missing
    /// value).
    pub fn resource(&self) -> Option<&T> {
        match &self.resource {
            Slot::Present(resource) => Some(resource),
            _ => None,
        }
    }

    /// Take the underlying resource out.
    pub fn into_resource(self) -> Option<T> {
        match self.resource {
            Slot::Present(resource) => Some(resource),
            _ => None,
        }
    }

    /// Whether the resource wraps a missing value.
    pub fn is_missing(&self) -> bool {
        matches!(self.resource, Slot::Missing)
    }

    /// Whether the resource wraps `null`.
    pub fn is_null(&self) -> bool {
        matches!(self.resource, Slot::Null)
    }

    /// The paginator behind the resource, if it is a paginated collection.
    pub fn pagination(&self) -> Option<&Pagination> {
        self.pagination.as_ref()
    }

    /// Add top-level data to the resource response.
    ///
    /// ```ignore
    /// UserResource::collection(users).additional(json!({"meta": {"key": "value"}}))
    /// ```
    pub fn additional(mut self, data: Value) -> Self {
        self.additional = data;
        self
    }

    /// The additional top-level data.
    pub fn get_additional(&self) -> &Value {
        &self.additional
    }

    /// Append the request's query string to the pagination links.
    pub fn preserve_query(mut self) -> Self {
        self.query = QueryParameters::Preserve;
        self
    }

    /// Append the given query string parameters to the pagination links.
    pub fn with_query(mut self, query: Value) -> Self {
        self.query = QueryParameters::Only(match query {
            Value::Object(map) => map,
            _ => Map::new(),
        });
        self
    }

    /// Resolve the resource to an array.
    pub fn resolve(&self, request: &Request) -> Value {
        match &self.resource {
            Slot::Present(resource) => resource.resolve(request),
            Slot::Null | Slot::Missing => Value::Array(Vec::new()),
        }
    }

    /// Transform the resource into a JSON response, for the current request.
    ///
    /// ```ignore
    /// UserResource::make(user).response().with_header("X-Value", "True")
    /// ```
    pub fn response(&self) -> Response {
        self.to_response(&request())
    }

    /// Transform the resource into a JSON response for the given request.
    pub fn to_response(&self, request: &Request) -> Response {
        with_request_sync(request.clone(), || self.build_response(request))
    }

    fn build_response(&self, request: &Request) -> Response {
        let resource = self.resource();
        let data = self.resolve(request);
        let with = resource
            .map(|resource| resource.with(request))
            .unwrap_or(Value::Null);
        let wrapper = T::wrapper();
        let force_wrapping = ResourceState::resolve().force_wrapping();

        let body = match self.pagination_with_query(request) {
            Some(pagination) => {
                let paginated = pagination.to_array();
                let default = pagination_information(&paginated);
                let information = match resource {
                    Some(resource) => resource.pagination_information(request, &paginated, default),
                    None => default,
                };
                let with = array_merge_recursive([information, with, self.additional.clone()]);
                wrap(data, with, Value::Null, wrapper.as_deref(), force_wrapping)
            }
            None => wrap(
                data,
                with,
                self.additional.clone(),
                wrapper.as_deref(),
                force_wrapping,
            ),
        };

        let mut response = Response::json(&body);
        if let Some(resource) = resource {
            resource.with_response(request, &mut response);
        }
        response
    }

    fn pagination_with_query(&self, request: &Request) -> Option<Pagination> {
        let pagination = self.pagination.clone()?;
        Some(match &self.query {
            QueryParameters::None => pagination,
            QueryParameters::Preserve => match request.query_all() {
                Value::Object(query) => pagination.appends(query),
                _ => pagination,
            },
            QueryParameters::Only(query) => pagination.appends(query.clone()),
        })
    }

    /// Convert the resource to JSON, for the current request.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "null".into())
    }

    /// Convert the resource to pretty printed JSON, for the current request.
    pub fn to_pretty_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "null".into())
    }
}

/// Nested resources resolve themselves with the current request; resources
/// wrapping `null` render as `null`, and missing ones are left out.
impl<T: JsonResource> Serialize for Resource<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.resource {
            Slot::Present(resource) => resource.resolve(&request()).serialize(serializer),
            Slot::Null => serializer.serialize_none(),
            Slot::Missing => MissingValue.serialize(serializer),
        }
    }
}

/// Return a resource from a route and it becomes a JSON response.
impl<T: JsonResource> IntoResponse for Resource<T> {
    fn into_response(self) -> Response {
        self.response()
    }
}

/// Models that know their default resource (Laravel's `#[UseResource]`).
///
/// ```ignore
/// impl UseResource for User {
///     type Resource = UserResource;
/// }
///
/// Route::get("/user/{id}", |Path(id): Path<i64>| async move {
///     Ok::<_, Error>(User::find_or_fail(id).await?.to_resource())
/// });
/// ```
pub trait UseResource: Sized {
    /// The model's default resource.
    type Resource: JsonResource<Model = Self>;

    /// Transform the model into its default resource.
    fn to_resource(self) -> Resource<Self::Resource> {
        Self::Resource::make(self)
    }
}
