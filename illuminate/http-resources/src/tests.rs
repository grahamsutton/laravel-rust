//! Resources end to end, without a database: plain serializable models,
//! resolved through resources, collections and paginators into responses.

use std::sync::Arc;

use indexmap::IndexMap;
use serde::Serialize;

use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_http::{IntoResponse, Request, Response, request, with_request};
use illuminate_pagination::{LengthAwarePaginator, Paginator, PaginatorOptions};
use illuminate_support::{Collection, Value, json};

use crate::*;

fn app() -> LocalInstanceGuard {
    let container = Arc::new(Container::new());
    HttpResourcesServiceProvider.register(&container);
    Container::set_local_instance(container)
}

fn get(uri: &str) -> Request {
    Request::create(uri, "GET")
}

// ----------------------------------------------------------------------
// Models and resources
// ----------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
struct Post {
    id: u64,
    title: String,
    #[serde(skip)]
    author: Option<Box<User>>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct User {
    id: u64,
    name: String,
    email: String,
    nickname: Option<String>,
    is_admin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    posts: Option<Vec<Post>>,
}

fn user(id: u64, name: &str) -> User {
    User {
        id,
        name: name.to_string(),
        email: format!("{}@laravel.com", name.to_lowercase()),
        ..User::default()
    }
}

fn post(id: u64, title: &str) -> Post {
    Post {
        id,
        title: title.to_string(),
        author: None,
    }
}

#[derive(Clone)]
struct PostResource(Post);

impl JsonResource for PostResource {
    type Model = Post;

    fn from_model(post: Post) -> Self {
        Self(post)
    }

    fn model(&self) -> &Post {
        &self.0
    }

    fn to_array(&self, _request: &Request) -> Value {
        json!({
            "id": self.0.id,
            "title": self.0.title,
            "author": self.when_loaded(&self.0.author).map(UserResource::make),
        })
    }
}

#[derive(Clone)]
struct UserResource(User);

impl JsonResource for UserResource {
    type Model = User;

    fn from_model(user: User) -> Self {
        Self(user)
    }

    fn model(&self) -> &User {
        &self.0
    }

    fn to_array(&self, request: &Request) -> Value {
        json!({
            "id": self.0.id,
            "name": self.0.name,
            "nickname": self.when_not_null(&self.0.nickname),
            "posts": PostResource::collection(self.when_loaded(&self.0.posts)),
            "secret": self.when(request.boolean("admin"), || "secret-value"),
            "admin": self.merge_when(self.0.is_admin, || json!({
                "first-secret": "value",
                "second-secret": "value",
            })),
        })
    }
}

/// A resource relying on the default `to_array`.
struct PlainResource(User);

impl JsonResource for PlainResource {
    type Model = User;

    fn from_model(user: User) -> Self {
        Self(user)
    }

    fn model(&self) -> &User {
        &self.0
    }
}

// ----------------------------------------------------------------------
// Single resources
// ----------------------------------------------------------------------

#[test]
fn resources_are_wrapped_in_data() {
    let _app = app();
    let response = UserResource::make(user(1, "Taylor")).to_response(&get("/"));

    assert_eq!(response.status_code(), 200);
    assert_eq!(response.header("content-type").unwrap(), "application/json");
    assert_eq!(
        response.json_body(),
        json!({"data": {"id": 1, "name": "Taylor"}})
    );
}

#[test]
fn new_is_an_alias_of_make() {
    let _app = app();
    let request = get("/");
    assert_eq!(
        UserResource::new(user(1, "Taylor")).resolve(&request),
        UserResource::make(user(1, "Taylor")).resolve(&request)
    );
}

#[test]
fn the_default_to_array_serializes_the_model() {
    let _app = app();
    let resolved = PlainResource::make(user(1, "Taylor")).resolve(&get("/"));
    assert_eq!(
        resolved,
        json!({
            "id": 1,
            "name": "Taylor",
            "email": "taylor@laravel.com",
            "nickname": null,
            "is_admin": false,
        })
    );
}

#[test]
fn conditional_attributes_are_included_when_their_condition_holds() {
    let _app = app();
    let mut taylor = user(1, "Taylor");
    taylor.nickname = Some("otwell".into());
    taylor.is_admin = true;

    let resolved = UserResource::make(taylor).resolve(&get("/?admin=1"));

    assert_eq!(
        resolved,
        json!({
            "id": 1,
            "name": "Taylor",
            "nickname": "otwell",
            "secret": "secret-value",
            "first-secret": "value",
            "second-secret": "value",
        })
    );
    let keys: Vec<&String> = resolved.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "id",
            "name",
            "nickname",
            "secret",
            "first-secret",
            "second-secret"
        ]
    );
}

#[test]
fn conditional_attributes_are_removed_otherwise() {
    let _app = app();
    let resolved = UserResource::make(user(1, "Taylor")).resolve(&get("/?admin=0"));
    assert_eq!(resolved, json!({"id": 1, "name": "Taylor"}));
}

#[test]
fn loaded_relationships_are_included() {
    let _app = app();
    let mut taylor = user(1, "Taylor");
    taylor.posts = Some(vec![post(1, "Hello"), post(2, "World")]);

    let resolved = UserResource::make(taylor).resolve(&get("/"));

    assert_eq!(
        resolved["posts"],
        json!([{"id": 1, "title": "Hello"}, {"id": 2, "title": "World"}])
    );
}

#[test]
fn nested_single_resources_resolve_in_place() {
    let _app = app();
    let mut hello = post(1, "Hello");
    hello.author = Some(Box::new(user(1, "Taylor")));

    let response = PostResource::make(hello).to_response(&get("/"));

    assert_eq!(
        response.json_body(),
        json!({"data": {"id": 1, "title": "Hello", "author": {"id": 1, "name": "Taylor"}}})
    );
}

#[test]
fn nested_resources_wrapping_null_render_as_null() {
    let _app = app();
    let data = json!({
        "author": UserResource::make(None::<User>),
        "posts": PostResource::collection(None::<Vec<Post>>),
        "missing": UserResource::make(PotentiallyMissing::<User>::Missing),
        "missing_posts": PostResource::collection(PotentiallyMissing::<Vec<Post>>::Missing),
    });
    assert_eq!(filter(data), json!({"author": null, "posts": null}));
}

#[test]
fn resources_can_be_made_from_references_boxes_and_options() {
    let _app = app();
    let request = get("/");
    let taylor = user(1, "Taylor");
    let boxed = Some(Box::new(user(2, "Abigail")));
    let expected = json!({"id": 1, "name": "Taylor"});

    assert_eq!(UserResource::make(&taylor).resolve(&request), expected);
    assert_eq!(
        UserResource::make(Box::new(taylor.clone())).resolve(&request),
        expected
    );
    assert_eq!(
        UserResource::make(Some(&taylor)).resolve(&request),
        expected
    );
    let field = Some(taylor.clone());
    assert_eq!(UserResource::make(&field).resolve(&request), expected);
    assert!(field.is_some());
    assert_eq!(
        UserResource::make(&boxed).resolve(&request),
        json!({"id": 2, "name": "Abigail"})
    );
    assert_eq!(
        UserResource::make(PotentiallyMissing::Present(&taylor)).resolve(&request),
        expected
    );
    assert!(UserResource::make(None::<&User>).is_null());
    assert!(UserResource::make(PotentiallyMissing::<Box<User>>::Missing).is_missing());
    assert_eq!(UserResource::make(&taylor).resource().unwrap().0.id, 1);
    assert_eq!(
        UserResource::make(&taylor).into_resource().unwrap().0.name,
        "Taylor"
    );
}

#[test]
fn null_resources_respond_with_an_empty_array() {
    let _app = app();
    let response = UserResource::make(None::<User>).to_response(&get("/"));
    assert_eq!(response.json_body(), json!({"data": []}));
}

#[test]
fn nested_resources_receive_the_request() {
    let _app = app();
    let mut hello = post(1, "Hello");
    hello.author = Some(Box::new(user(1, "Taylor")));

    let resolved = PostResource::make(hello).resolve(&get("/?admin=1"));

    assert_eq!(resolved["author"]["secret"], json!("secret-value"));
}

#[test]
fn the_request_is_current_while_resolving() {
    let _app = app();

    struct EchoResource(Value);

    impl JsonResource for EchoResource {
        type Model = Value;

        fn from_model(value: Value) -> Self {
            Self(value)
        }

        fn model(&self) -> &Value {
            &self.0
        }

        fn to_array(&self, _request: &Request) -> Value {
            json!({"path": request().path()})
        }
    }

    let resolved = EchoResource::make(json!(null)).resolve(&get("/users/1"));
    assert_eq!(resolved, json!({"path": "users/1"}));
}

#[tokio::test]
async fn responses_use_the_current_request() {
    let _app = app();
    let response = with_request(get("/?admin=1"), async {
        UserResource::make(user(1, "Taylor")).into_response()
    })
    .await;
    assert_eq!(
        response.json_body()["data"]["secret"],
        json!("secret-value")
    );
}

// ----------------------------------------------------------------------
// The other conditional helpers
// ----------------------------------------------------------------------

struct HelperResource(User);

impl JsonResource for HelperResource {
    type Model = User;

    fn from_model(user: User) -> Self {
        Self(user)
    }

    fn model(&self) -> &User {
        &self.0
    }

    fn to_array(&self, _request: &Request) -> Value {
        let user = &self.0;
        json!({
            "unless": self.unless(user.is_admin, || "not an admin"),
            "when_null": self.when_null(&user.nickname),
            "nickname": self.when_not_null(user.nickname.clone()),
            "fallback": self.when(false, || "x").unwrap_or("fallback"),
            "merged": self.merge(json!({"merged": true})),
            "unless_admin": self.merge_unless(user.is_admin, || json!({"guest": true})),
            "only": self.attributes(&["email", "id"]),
            "posts": self.when_loaded(&user.posts).map(|posts| posts.len()),
            "list": [1, self.when(user.is_admin, || 2), self.merge([3, 4])],
        })
    }
}

#[test]
fn every_helper_resolves_like_laravel() {
    let _app = app();
    let request = get("/");

    let guest = HelperResource::make(user(1, "Taylor")).resolve(&request);
    assert_eq!(
        guest,
        json!({
            "unless": "not an admin",
            "when_null": null,
            "fallback": "fallback",
            "merged": true,
            "guest": true,
            "id": 1,
            "email": "taylor@laravel.com",
            "list": [1, 3, 4],
        })
    );

    let mut admin = user(2, "Abigail");
    admin.is_admin = true;
    admin.nickname = Some("abby".into());
    admin.posts = Some(vec![post(1, "Hello")]);
    let resolved = HelperResource::make(admin).resolve(&request);
    assert_eq!(
        resolved,
        json!({
            "nickname": "abby",
            "fallback": "fallback",
            "merged": true,
            "id": 2,
            "email": "abigail@laravel.com",
            "posts": 1,
            "list": [1, 2, 3, 4],
        })
    );
}

// ----------------------------------------------------------------------
// Top-level data and the response
// ----------------------------------------------------------------------

struct MetaResource(Value);

impl JsonResource for MetaResource {
    type Model = Value;

    fn from_model(value: Value) -> Self {
        Self(value)
    }

    fn model(&self) -> &Value {
        &self.0
    }

    fn with(&self, _request: &Request) -> Value {
        json!({"meta": {"key": "value"}})
    }

    fn with_response(&self, _request: &Request, response: &mut Response) {
        response.set_header("X-Value", "True");
    }
}

#[test]
fn with_and_additional_data_are_merged_into_the_response() {
    let _app = app();
    let response = MetaResource::make(json!({"id": 1}))
        .additional(json!({"meta": {"extra": true}, "version": 2}))
        .to_response(&get("/"));

    assert_eq!(
        response.json_body(),
        json!({
            "data": {"id": 1},
            "meta": {"key": "value", "extra": true},
            "version": 2,
        })
    );
    assert_eq!(response.header("x-value").unwrap(), "True");
}

#[test]
fn additional_replaces_previous_additional_data() {
    let _app = app();
    let resource = MetaResource::make(json!({"id": 1}))
        .additional(json!({"first": 1}))
        .additional(json!({"second": 2}));
    assert_eq!(resource.get_additional(), &json!({"second": 2}));
}

#[test]
fn nested_resources_ignore_their_top_level_data() {
    let _app = app();
    let data = json!({"nested": MetaResource::make(json!({"id": 1})).additional(json!({"x": 1}))});
    assert_eq!(filter(data), json!({"nested": {"id": 1}}));
}

#[test]
fn the_response_may_be_customized() {
    let _app = app();
    let response = UserResource::make(user(1, "Taylor"))
        .response()
        .with_header("X-Custom", "Yes")
        .with_status(202);
    assert_eq!(response.status_code(), 202);
    assert_eq!(response.header("x-custom").unwrap(), "Yes");

    let created = (201, UserResource::make(user(1, "Taylor"))).into_response();
    assert_eq!(created.status_code(), 201);
    assert_eq!(created.json_body()["data"]["id"], json!(1));
}

#[test]
fn resources_convert_to_json() {
    let _app = app();
    let resource = UserResource::make(user(1, "Taylor"));
    assert_eq!(resource.to_json(), r#"{"id":1,"name":"Taylor"}"#);
    assert!(resource.to_pretty_json().contains("\n  \"id\": 1"));
    assert_eq!(UserResource::make(None::<User>).to_json(), "null");
}

#[test]
fn resources_can_be_cloned() {
    let _app = app();
    let resource = UserResource::make(user(1, "Taylor")).additional(json!({"a": 1}));
    let copy = resource.clone();
    assert_eq!(copy.resolve(&get("/")), resource.resolve(&get("/")));
}

// ----------------------------------------------------------------------
// Wrapping
// ----------------------------------------------------------------------

struct WrappedResource(Value);

impl JsonResource for WrappedResource {
    type Model = Value;

    fn from_model(value: Value) -> Self {
        Self(value)
    }

    fn model(&self) -> &Value {
        &self.0
    }

    fn wrapper() -> Option<String> {
        Some("user".into())
    }
}

#[test]
fn resources_may_be_wrapped_in_their_own_key() {
    let _app = app();
    let body = WrappedResource::make(json!({"id": 1}))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body, json!({"user": {"id": 1}}));

    Resource::without_wrapping();
    let body = WrappedResource::make(json!({"id": 1}))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body, json!({"user": {"id": 1}}));
}

#[test]
fn wrapping_can_be_changed_per_resource_at_runtime() {
    let _app = app();
    PlainResource::wrap("user");
    UserResource::without_wrapping();

    let plain = PlainResource::make(user(1, "Taylor"))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(plain["user"]["id"], json!(1));

    let body = UserResource::make(user(1, "Taylor"))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body, json!({"id": 1, "name": "Taylor"}));

    let other = MetaResource::make(json!({"id": 1}))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(other["data"], json!({"id": 1}));
}

#[test]
fn wrapping_can_be_disabled_globally() {
    let _app = app();
    Resource::without_wrapping();
    assert_eq!(Resource::wrapper(), None);

    let body = UserResource::make(user(1, "Taylor"))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body, json!({"id": 1, "name": "Taylor"}));

    // Top-level information still needs a home, so the data is wrapped.
    let body = MetaResource::make(json!({"id": 1}))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body["data"], json!({"id": 1}));
    assert_eq!(body["meta"], json!({"key": "value"}));

    Resource::wrap("payload");
    let body = UserResource::make(user(1, "Taylor"))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body["payload"]["id"], json!(1));

    Resource::flush_state();
    let body = UserResource::make(user(1, "Taylor"))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body["data"]["id"], json!(1));
}

#[test]
fn wrapping_settings_are_isolated_per_container() {
    let _first = app();
    Resource::without_wrapping();
    UserResource::wrap("person");
    {
        let _second = app();
        assert_eq!(Resource::wrapper().as_deref(), Some("data"));
        assert_eq!(UserResource::wrapper().as_deref(), Some("data"));
    }
    assert_eq!(Resource::wrapper(), None);
    assert_eq!(UserResource::wrapper().as_deref(), Some("person"));
}

struct AlreadyWrapped(Value);

impl JsonResource for AlreadyWrapped {
    type Model = Value;

    fn from_model(value: Value) -> Self {
        Self(value)
    }

    fn model(&self) -> &Value {
        &self.0
    }

    fn to_array(&self, _request: &Request) -> Value {
        json!({"data": self.0, "links": {"self": "link-value"}})
    }
}

#[test]
fn resources_are_never_double_wrapped() {
    let _app = app();
    let body = AlreadyWrapped::make(json!([1, 2]))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(
        body,
        json!({"data": [1, 2], "links": {"self": "link-value"}})
    );
}

#[test]
fn wrapping_can_be_forced() {
    let _app = app();
    Resource::force_wrapping(true);
    let body = AlreadyWrapped::make(json!([1]))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(
        body,
        json!({"data": {"data": [1], "links": {"self": "link-value"}}})
    );
}

// ----------------------------------------------------------------------
// Anonymous collections
// ----------------------------------------------------------------------

#[test]
fn collections_are_wrapped_in_data() {
    let _app = app();
    let users = vec![user(1, "Taylor"), user(2, "Abigail")];
    let response = UserResource::collection(users).to_response(&get("/"));
    assert_eq!(
        response.json_body(),
        json!({"data": [{"id": 1, "name": "Taylor"}, {"id": 2, "name": "Abigail"}]})
    );
}

#[test]
fn collections_can_be_made_from_many_shapes() {
    let _app = app();
    let request = get("/");
    let users = vec![user(1, "Taylor")];
    let collection = Collection::from(users.clone());
    let expected = json!([{"id": 1, "name": "Taylor"}]);

    assert_eq!(
        UserResource::collection(users.clone()).resolve(&request),
        expected
    );
    assert_eq!(UserResource::collection(&users).resolve(&request), expected);
    assert_eq!(
        UserResource::collection(users.as_slice()).resolve(&request),
        expected
    );
    assert_eq!(
        UserResource::collection(&collection).resolve(&request),
        expected
    );
    assert_eq!(
        UserResource::collection(collection.clone()).resolve(&request),
        expected
    );
    assert_eq!(
        UserResource::collection(Some(users.clone())).resolve(&request),
        expected
    );
    assert_eq!(
        UserResource::collection(Some(&users)).resolve(&request),
        expected
    );
    let field = Some(users.clone());
    assert_eq!(UserResource::collection(&field).resolve(&request), expected);
    assert!(field.is_some());
    assert_eq!(
        UserResource::collection(PotentiallyMissing::Present(&users)).resolve(&request),
        expected
    );
    assert_eq!(
        UserResource::collection(Vec::<User>::new()).resolve(&request),
        json!([])
    );
    assert_eq!(UserResource::collection(users).count(), 1);
}

#[test]
fn collections_expose_their_resources() {
    let _app = app();
    let collection = UserResource::collection(vec![user(1, "Taylor"), user(2, "Abigail")]);
    let resources = collection.collection().unwrap();
    assert_eq!(resources.len(), 2);
    assert!(!resources.is_empty());
    assert_eq!(resources.first().unwrap().0.name, "Taylor");
    let ids: Vec<u64> = resources.iter().map(|resource| resource.0.id).collect();
    assert_eq!(ids, [1, 2]);
    assert_eq!(resources.all().len(), 2);
    assert_eq!(resources.keys(), None);
}

#[test]
fn keyed_collections_are_reindexed_unless_keys_are_preserved() {
    let _app = app();
    let request = get("/");
    let keyed: IndexMap<u64, User> = [(5, user(5, "Taylor")), (9, user(9, "Abigail"))].into();

    assert_eq!(
        UserResource::collection(keyed.clone()).resolve(&request),
        json!([{"id": 5, "name": "Taylor"}, {"id": 9, "name": "Abigail"}])
    );
    assert_eq!(
        UserResource::collection(keyed)
            .preserve_keys()
            .resolve(&request),
        json!({"5": {"id": 5, "name": "Taylor"}, "9": {"id": 9, "name": "Abigail"}})
    );
}

#[test]
fn string_keyed_collections_keep_their_keys() {
    let _app = app();
    let keyed: IndexMap<String, User> = [("taylor".to_string(), user(1, "Taylor"))].into();
    assert_eq!(
        UserResource::collection(keyed).resolve(&get("/")),
        json!({"taylor": {"id": 1, "name": "Taylor"}})
    );
}

struct KeyedResource(User);

impl JsonResource for KeyedResource {
    type Model = User;

    fn from_model(user: User) -> Self {
        Self(user)
    }

    fn model(&self) -> &User {
        &self.0
    }

    fn to_array(&self, _request: &Request) -> Value {
        json!({"id": self.0.id})
    }

    fn preserve_keys() -> bool {
        true
    }
}

#[test]
fn resources_may_always_preserve_collection_keys() {
    let _app = app();
    let keyed: IndexMap<u64, User> = [(3, user(3, "Taylor"))].into();
    let collection = KeyedResource::collection(keyed);
    assert!(collection.collection().unwrap().preserves_keys());
    assert_eq!(collection.resolve(&get("/")), json!({"3": {"id": 3}}));
}

#[test]
fn collection_additional_data_is_merged() {
    let _app = app();
    let body = UserResource::collection(vec![user(1, "Taylor")])
        .additional(json!({"meta": {"key": "value"}}))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(
        body,
        json!({"data": [{"id": 1, "name": "Taylor"}], "meta": {"key": "value"}})
    );
}

#[test]
fn collections_are_unwrapped_without_wrapping() {
    let _app = app();
    Resource::without_wrapping();
    let body = UserResource::collection(vec![user(1, "Taylor")])
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body, json!([{"id": 1, "name": "Taylor"}]));
}

// ----------------------------------------------------------------------
// Dedicated collections
// ----------------------------------------------------------------------

struct UserCollection(Resources<UserResource>);

impl ResourceCollection for UserCollection {
    type Collects = UserResource;

    fn from_collection(collection: Resources<UserResource>) -> Self {
        Self(collection)
    }

    fn collection(&self) -> &Resources<UserResource> {
        &self.0
    }

    fn to_array(&self, _request: &Request) -> Value {
        json!({
            "data": self.0,
            "links": {"self": "link-value"},
        })
    }

    fn with(&self, _request: &Request) -> Value {
        json!({"meta": {"count": self.0.count()}})
    }
}

/// A collection relying on the defaults.
struct MemberCollection(Resources<UserResource>);

impl ResourceCollection for MemberCollection {
    type Collects = UserResource;

    fn from_collection(collection: Resources<UserResource>) -> Self {
        Self(collection)
    }

    fn collection(&self) -> &Resources<UserResource> {
        &self.0
    }

    fn preserve_keys() -> bool {
        true
    }
}

#[test]
fn dedicated_collections_have_their_own_array_and_meta() {
    let _app = app();
    let body = UserCollection::make(vec![user(1, "Taylor"), user(2, "Abigail")])
        .to_response(&get("/"))
        .json_body();
    assert_eq!(
        body,
        json!({
            "data": [{"id": 1, "name": "Taylor"}, {"id": 2, "name": "Abigail"}],
            "links": {"self": "link-value"},
            "meta": {"count": 2},
        })
    );
}

#[test]
fn dedicated_collections_default_to_resolving_every_resource() {
    let _app = app();
    let keyed: IndexMap<u64, User> = [(4, user(4, "Taylor"))].into();
    let body = MemberCollection::new(keyed)
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body, json!({"data": {"4": {"id": 4, "name": "Taylor"}}}));
}

#[test]
fn collections_serialize_their_keys_as_is_like_laravel_collections() {
    let _app = app();
    let keyed: IndexMap<u64, User> = [(4, user(4, "Taylor"))].into();
    let body = UserCollection::make(keyed)
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body["data"], json!({"4": {"id": 4, "name": "Taylor"}}));

    let sequential: IndexMap<u64, User> = [(0, user(4, "Taylor"))].into();
    let body = UserCollection::make(sequential)
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body["data"], json!([{"id": 4, "name": "Taylor"}]));
}

#[test]
fn dedicated_collections_may_be_nested() {
    let _app = app();
    let data = json!({"users": UserCollection::make(vec![user(1, "Taylor")])});
    assert_eq!(
        filter(data),
        json!({"users": {"data": [{"id": 1, "name": "Taylor"}], "links": {"self": "link-value"}}})
    );
}

#[test]
fn dedicated_collections_can_be_wrapped_differently() {
    let _app = app();
    MemberCollection::wrap("members");
    let body = MemberCollection::make(vec![user(1, "Taylor")])
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body, json!({"members": [{"id": 1, "name": "Taylor"}]}));
}

// ----------------------------------------------------------------------
// Pagination
// ----------------------------------------------------------------------

fn paginator(page: u64) -> LengthAwarePaginator<User> {
    let all = [user(1, "Taylor"), user(2, "Abigail"), user(3, "Jess")];
    let items: Vec<User> = all
        .iter()
        .skip(((page - 1) * 2) as usize)
        .take(2)
        .cloned()
        .collect();
    LengthAwarePaginator::new(
        items,
        3,
        2,
        page,
        PaginatorOptions::path("http://example.com/users"),
    )
}

#[test]
fn paginated_collections_have_links_and_meta() {
    let _app = app();
    let body = UserResource::collection(paginator(1))
        .to_response(&get("/users"))
        .json_body();

    assert_eq!(
        body,
        json!({
            "data": [{"id": 1, "name": "Taylor"}, {"id": 2, "name": "Abigail"}],
            "links": {
                "first": "http://example.com/users?page=1",
                "last": "http://example.com/users?page=2",
                "prev": null,
                "next": "http://example.com/users?page=2",
            },
            "meta": {
                "current_page": 1,
                "from": 1,
                "last_page": 2,
                "links": [
                    {"url": null, "label": "&laquo; Previous", "page": null, "active": false},
                    {"url": "http://example.com/users?page=1", "label": "1", "page": 1, "active": true},
                    {"url": "http://example.com/users?page=2", "label": "2", "page": 2, "active": false},
                    {"url": "http://example.com/users?page=2", "label": "Next &raquo;", "page": 2, "active": false},
                ],
                "path": "http://example.com/users",
                "per_page": 2,
                "to": 2,
                "total": 3,
            },
        })
    );
    let keys: Vec<String> = body.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["data", "links", "meta"]);
}

#[test]
fn paginated_collections_are_wrapped_even_without_wrapping() {
    let _app = app();
    Resource::without_wrapping();
    let body = UserResource::collection(paginator(2))
        .to_response(&get("/users"))
        .json_body();
    assert_eq!(body["data"], json!([{"id": 3, "name": "Jess"}]));
    assert_eq!(
        body["links"]["prev"],
        json!("http://example.com/users?page=1")
    );
    assert_eq!(body["links"]["next"], json!(null));
    assert_eq!(body["meta"]["current_page"], json!(2));
}

#[test]
fn simple_paginators_have_links_and_meta() {
    let _app = app();
    let items = vec![user(1, "Taylor"), user(2, "Abigail"), user(3, "Jess")];
    let paginator = Paginator::new(items, 2, 1, PaginatorOptions::path("/users"));
    let body = UserResource::collection(paginator)
        .to_response(&get("/users"))
        .json_body();
    assert_eq!(
        body,
        json!({
            "data": [{"id": 1, "name": "Taylor"}, {"id": 2, "name": "Abigail"}],
            "links": {"first": "/users?page=1", "last": null, "prev": null, "next": "/users?page=2"},
            "meta": {
                "current_page": 1,
                "current_page_url": "/users?page=1",
                "from": 1,
                "path": "/users",
                "per_page": 2,
                "to": 2,
            },
        })
    );
}

#[test]
fn pagination_links_may_preserve_the_query_string() {
    let _app = app();
    let body = UserResource::collection(paginator(1))
        .preserve_query()
        .to_response(&get("/users?sort=name&page=1"))
        .json_body();
    assert_eq!(
        body["links"]["next"],
        json!("http://example.com/users?sort=name&page=2")
    );

    let body = UserResource::collection(paginator(1))
        .with_query(json!({"filter": "active"}))
        .to_response(&get("/users?sort=name"))
        .json_body();
    assert_eq!(
        body["links"]["next"],
        json!("http://example.com/users?filter=active&page=2")
    );
}

struct CustomPagination(Resources<UserResource>);

impl ResourceCollection for CustomPagination {
    type Collects = UserResource;

    fn from_collection(collection: Resources<UserResource>) -> Self {
        Self(collection)
    }

    fn collection(&self) -> &Resources<UserResource> {
        &self.0
    }

    fn pagination_information(
        &self,
        _request: &Request,
        paginated: &Value,
        mut default: Value,
    ) -> Value {
        default["links"]["custom"] = json!("https://example.com");
        default["meta"] = json!({"page": paginated["current_page"]});
        default
    }
}

#[test]
fn pagination_information_can_be_customized() {
    let _app = app();
    let body = CustomPagination::make(paginator(1))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(body["links"]["custom"], json!("https://example.com"));
    assert_eq!(
        body["links"]["first"],
        json!("http://example.com/users?page=1")
    );
    assert_eq!(body["meta"], json!({"page": 1}));
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
}

#[test]
fn collection_links_are_merged_with_the_paginator_links() {
    let _app = app();
    let body = UserCollection::make(paginator(1))
        .additional(json!({"meta": {"key": "value"}}))
        .to_response(&get("/"))
        .json_body();
    assert_eq!(
        body["links"],
        json!({
            "self": "link-value",
            "first": "http://example.com/users?page=1",
            "last": "http://example.com/users?page=2",
            "prev": null,
            "next": "http://example.com/users?page=2",
        })
    );
    assert_eq!(body["meta"]["count"], json!(2));
    assert_eq!(body["meta"]["key"], json!("value"));
    assert_eq!(body["meta"]["total"], json!(3));
}

#[test]
fn nested_paginated_collections_are_plain_lists() {
    let _app = app();
    let data = json!({"users": UserResource::collection(paginator(1))});
    assert_eq!(
        filter(data),
        json!({"users": [{"id": 1, "name": "Taylor"}, {"id": 2, "name": "Abigail"}]})
    );
}

#[test]
fn the_pagination_is_available_on_the_resource() {
    let _app = app();
    let collection = UserResource::collection(paginator(1));
    let pagination = collection.pagination().unwrap();
    assert_eq!(pagination.to_array()["total"], json!(3));
    assert!(pagination.to_array().get("data").is_none());
    assert!(
        UserResource::collection(vec![user(1, "Taylor")])
            .pagination()
            .is_none()
    );
}

// ----------------------------------------------------------------------
// Default resources
// ----------------------------------------------------------------------

impl UseResource for User {
    type Resource = UserResource;
}

#[test]
fn models_may_name_their_default_resource() {
    let _app = app();
    let request = get("/");
    assert_eq!(
        user(1, "Taylor").to_resource().resolve(&request),
        json!({"id": 1, "name": "Taylor"})
    );
    assert_eq!(
        vec![user(1, "Taylor")]
            .to_resource_collection()
            .resolve(&request),
        json!([{"id": 1, "name": "Taylor"}])
    );
    assert_eq!(
        Collection::from(vec![user(2, "Abigail")])
            .to_resource_collection()
            .resolve(&request),
        json!([{"id": 2, "name": "Abigail"}])
    );
    let body = paginator(1)
        .to_resource_collection()
        .to_response(&request)
        .json_body();
    assert_eq!(body["meta"]["total"], json!(3));
    let simple = Paginator::new(vec![user(1, "Taylor")], 2, 1, PaginatorOptions::path("/"));
    assert_eq!(simple.to_resource_collection().count(), 1);
}
