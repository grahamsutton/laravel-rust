//! Resources over real Eloquent models (in-memory SQLite), returned from
//! routes dispatched through the router.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_database::eloquent::*;
use illuminate_database::{DatabaseManager, DatabaseServiceProvider};
use illuminate_http::{Request, Response};
use illuminate_http_resources::HttpResourcesServiceProvider;
use illuminate_http_resources::prelude::*;
use illuminate_routing::{Path, Route, RoutingServiceProvider};
use illuminate_support::{Error, Value, json};

// ----------------------------------------------------------------------
// The application
// ----------------------------------------------------------------------

async fn app() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({"database": {
        "default": "sqlite",
        "connections": {
            "sqlite": {"driver": "sqlite", "database": ":memory:", "foreign_key_constraints": false},
        },
    }})));
    DatabaseServiceProvider.register(&container);
    RoutingServiceProvider.register(&container);
    HttpResourcesServiceProvider.register(&container);
    migrate().await;
    (container, guard)
}

async fn migrate() {
    let schema = DatabaseManager::resolve()
        .default_connection()
        .get_schema_builder();
    schema
        .create("users", |table| {
            table.id();
            table.string("name");
            table.string("email");
            table.string("password").default("");
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("posts", |table| {
            table.id();
            table.foreign_id("user_id");
            table.string("title");
            table.integer("votes").default(0);
        })
        .await
        .unwrap();
    schema
        .create("roles", |table| {
            table.id();
            table.string("name");
        })
        .await
        .unwrap();
    schema
        .create("role_user", |table| {
            table.foreign_id("role_id");
            table.foreign_id("user_id");
            table.string("expires_at").nullable();
        })
        .await
        .unwrap();
}

async fn seed() -> (User, User) {
    let taylor = User::create(
        json!({"name": "Taylor", "email": "taylor@laravel.com", "password": "secret"}),
    )
    .await
    .unwrap();
    let abigail = User::create(json!({"name": "Abigail", "email": "abigail@laravel.com"}))
        .await
        .unwrap();
    Post::create(json!({"user_id": taylor.id, "title": "Hello", "votes": 3}))
        .await
        .unwrap();
    Post::create(json!({"user_id": taylor.id, "title": "World", "votes": 4}))
        .await
        .unwrap();
    let admin = Role::create(json!({"name": "admin"})).await.unwrap();
    taylor
        .roles()
        .attach_with(vec![admin.id], json!({"expires_at": "2030-01-01"}))
        .await
        .unwrap();
    (taylor, abigail)
}

async fn send(uri: &str, method: &str) -> Response {
    Route::router().dispatch(Request::create(uri, method)).await
}

async fn get(uri: &str) -> Value {
    let response = send(uri, "GET").await;
    assert_eq!(response.status_code(), 200, "{}", response.content_string());
    assert_eq!(response.header("content-type").unwrap(), "application/json");
    response.json_body()
}

// ----------------------------------------------------------------------
// Models
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Default, Model)]
#[fillable(name, email, password)]
#[hidden(password)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub password: String,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,

    #[computed]
    pub posts_count: Option<i64>,
    #[computed]
    pub posts_sum_votes: Option<i64>,
    #[computed]
    pub posts_exists: Option<bool>,

    #[relation]
    pub posts: Option<Vec<Post>>,
    #[relation]
    pub roles: Option<Vec<Role>>,
}

impl User {
    pub fn posts(&self) -> HasMany<Self, Post> {
        self.has_many()
    }

    pub fn roles(&self) -> BelongsToMany<Self, Role> {
        self.belongs_to_many().with_pivot(["expires_at"])
    }
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(user_id, title, votes)]
#[without_timestamps]
pub struct Post {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub votes: i64,

    #[relation]
    pub user: Option<Box<User>>,
}

impl Post {
    pub fn user(&self) -> BelongsTo<Self, User> {
        self.belongs_to()
    }
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(name)]
#[without_timestamps]
pub struct Role {
    pub id: i64,
    pub name: String,

    #[computed]
    pub pivot: Option<Value>,
}

// ----------------------------------------------------------------------
// Resources
// ----------------------------------------------------------------------

pub struct UserResource(pub User);

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
            "email": self.when_has("email"),
            "secret": self.when(request.boolean("admin"), || "secret-value"),
            "posts": PostResource::collection(self.when_loaded(&self.0.posts)),
            "posts_count": self.when_counted("posts"),
            "votes": self.when_aggregated("posts", "votes", "sum"),
            "has_posts": self.when_exists_loaded("posts"),
            "roles": self.when_loaded("roles"),
            "created_at": self.0.created_at,
        })
    }
}

impl UseResource for User {
    type Resource = UserResource;
}

pub struct PostResource(pub Post);

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
            "author": self.when_loaded(&self.0.user).map(UserResource::make),
        })
    }
}

pub struct RoleResource(pub Role);

impl JsonResource for RoleResource {
    type Model = Role;

    fn from_model(role: Role) -> Self {
        Self(role)
    }

    fn model(&self) -> &Role {
        &self.0
    }

    fn to_array(&self, _request: &Request) -> Value {
        json!({
            "id": self.0.id,
            "name": self.0.name,
            "expires_at": self.when_pivot_loaded("role_user", |pivot| pivot["expires_at"].clone()),
            "subscription": self.when_pivot_loaded_as("subscription", "role_user", |pivot| pivot),
        })
    }
}

/// Relies on the model's own serialization.
pub struct PlainUserResource(pub User);

impl JsonResource for PlainUserResource {
    type Model = User;

    fn from_model(user: User) -> Self {
        Self(user)
    }

    fn model(&self) -> &User {
        &self.0
    }
}

pub struct UserCollection(pub Resources<UserResource>);

impl ResourceCollection for UserCollection {
    type Collects = UserResource;

    fn from_collection(collection: Resources<UserResource>) -> Self {
        Self(collection)
    }

    fn collection(&self) -> &Resources<UserResource> {
        &self.0
    }

    fn to_array(&self, _request: &Request) -> Value {
        json!({"data": self.0, "links": {"self": "link-value"}})
    }

    fn with(&self, _request: &Request) -> Value {
        json!({"meta": {"key": "value"}})
    }
}

// ----------------------------------------------------------------------
// Routes
// ----------------------------------------------------------------------

fn routes() {
    Route::get("/users/{id}", |Path(id): Path<i64>| async move {
        Ok::<_, Error>(UserResource::make(User::find_or_fail(id).await?))
    });
    Route::get("/users/{id}/plain", |Path(id): Path<i64>| async move {
        Ok::<_, Error>(PlainUserResource::make(User::find_or_fail(id).await?))
    });
    Route::get("/users/{id}/default", |Path(id): Path<i64>| async move {
        Ok::<_, Error>(User::find_or_fail(id).await?.to_resource())
    });
    Route::get("/users/{id}/roles", |Path(id): Path<i64>| async move {
        let user = User::find_or_fail(id).await?;
        Ok::<_, Error>(RoleResource::collection(user.roles().get().await?))
    });
    Route::get("/users", || async {
        Ok::<_, Error>(UserResource::collection(
            User::with(["posts", "roles"])
                .order_by("id", "asc")
                .get()
                .await?,
        ))
    });
    Route::get("/users-counted", || async {
        let users = User::with_count("posts")
            .with_sum("posts", "votes")
            .with_exists("posts")
            .order_by("id", "asc")
            .get()
            .await?;
        Ok::<_, Error>(users.to_resource_collection())
    });
    Route::get("/users-paginated", |request: Request| async move {
        let page = request.integer_or("page", 1).max(1) as u64;
        let users = User::query()
            .order_by("id", "asc")
            .paginate_with(1, "page", Some(page))
            .await?
            .with_path(request.url());
        Ok::<_, Error>(UserResource::collection(users).preserve_query())
    });
    Route::get("/users-collection", || async {
        let users = User::query()
            .order_by("id", "asc")
            .paginate_with(1, "page", Some(1))
            .await?;
        Ok::<_, Error>(UserCollection::make(
            users.with_path("http://localhost/users-collection"),
        ))
    });
    Route::post("/users", |request: Request| async move {
        let user = User::create(json!({
            "name": request.string("name"),
            "email": request.string("email"),
        }))
        .await?;
        Ok::<_, Error>((201, UserResource::make(user)))
    });
    Route::get("/posts/{id}", |Path(id): Path<i64>| async move {
        Ok::<_, Error>(PostResource::make(
            Post::with("user").find_or_fail(id).await?,
        ))
    });
    Route::get("/headers/{id}", |Path(id): Path<i64>| async move {
        let user = User::find_or_fail(id).await?;
        Ok::<_, Error>(
            UserResource::make(user)
                .response()
                .with_header("X-Value", "True"),
        )
    });
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[tokio::test]
async fn a_resource_is_returned_from_a_route() {
    let _app = app().await;
    let (taylor, _) = seed().await;
    routes();

    let body = get(&format!("/users/{}", taylor.id)).await;
    let data = &body["data"];

    assert_eq!(data["id"], json!(taylor.id));
    assert_eq!(data["name"], json!("Taylor"));
    assert_eq!(data["email"], json!("taylor@laravel.com"));
    assert!(data["created_at"].as_str().unwrap().ends_with('Z'));
    for missing in [
        "secret",
        "posts",
        "posts_count",
        "votes",
        "has_posts",
        "roles",
    ] {
        assert!(data.get(missing).is_none(), "{missing} should be missing");
    }

    let body = get(&format!("/users/{}?admin=1", taylor.id)).await;
    assert_eq!(body["data"]["secret"], json!("secret-value"));
}

#[tokio::test]
async fn missing_models_are_not_found() {
    let _app = app().await;
    routes();
    let response = send("/users/999", "GET").await;
    assert!(!response.is_successful());
}

#[tokio::test]
async fn the_default_array_respects_hidden_attributes() {
    let _app = app().await;
    let (taylor, _) = seed().await;
    routes();

    let body = get(&format!("/users/{}/plain", taylor.id)).await;
    let data = body["data"].as_object().unwrap();
    assert_eq!(data["name"], json!("Taylor"));
    assert!(!data.contains_key("password"));
    assert!(!data.contains_key("posts"));
}

#[tokio::test]
async fn models_may_use_their_default_resource() {
    let _app = app().await;
    let (taylor, _) = seed().await;
    routes();

    let body = get(&format!("/users/{}/default", taylor.id)).await;
    assert_eq!(body["data"]["name"], json!("Taylor"));
    assert!(body["data"].get("secret").is_none());
}

#[tokio::test]
async fn loaded_relationships_are_included() {
    let _app = app().await;
    seed().await;
    routes();

    let body = get("/users").await;
    let users = body["data"].as_array().unwrap();
    assert_eq!(users.len(), 2);

    assert_eq!(
        users[0]["posts"],
        json!([{"id": 1, "title": "Hello"}, {"id": 2, "title": "World"}])
    );
    let roles = users[0]["roles"].as_array().unwrap();
    assert_eq!(roles[0]["name"], json!("admin"));
    assert_eq!(roles[0]["pivot"]["expires_at"], json!("2030-01-01"));

    assert_eq!(users[1]["posts"], json!([]));
    assert_eq!(users[1]["roles"], json!([]));
}

#[tokio::test]
async fn belongs_to_relationships_nest_their_resource() {
    let _app = app().await;
    seed().await;
    routes();

    let body = get("/posts/1").await;
    assert_eq!(body["data"]["title"], json!("Hello"));
    assert_eq!(body["data"]["author"]["name"], json!("Taylor"));
    assert_eq!(body["data"]["author"]["email"], json!("taylor@laravel.com"));
    assert!(body["data"]["author"].get("posts").is_none());

    let post = Post::find_or_fail(1).await.unwrap();
    let resolved = PostResource::make(post).resolve(&Request::default());
    assert_eq!(resolved, json!({"id": 1, "title": "Hello"}));
}

#[tokio::test]
async fn counts_and_aggregates_are_included_when_loaded() {
    let _app = app().await;
    seed().await;
    routes();

    let body = get("/users-counted").await;
    let users = body["data"].as_array().unwrap();
    assert_eq!(users[0]["posts_count"], json!(2));
    assert_eq!(users[0]["votes"], json!(7));
    assert_eq!(users[0]["has_posts"], json!(true));

    assert_eq!(users[1]["posts_count"], json!(0));
    assert!(
        users[1].get("votes").is_none(),
        "the sum of no posts is null"
    );
    assert_eq!(users[1]["has_posts"], json!(false));
}

#[tokio::test]
async fn pivot_information_is_included_when_loaded_through_the_pivot() {
    let _app = app().await;
    let (taylor, _) = seed().await;
    routes();

    let body = get(&format!("/users/{}/roles", taylor.id)).await;
    assert_eq!(
        body,
        json!({"data": [{"id": 1, "name": "admin", "expires_at": "2030-01-01"}]})
    );

    let role = Role::find_or_fail(1).await.unwrap();
    let resolved = RoleResource::make(role).resolve(&Request::default());
    assert_eq!(resolved, json!({"id": 1, "name": "admin"}));
}

#[tokio::test]
async fn paginated_models_have_links_and_meta() {
    let _app = app().await;
    seed().await;
    routes();

    let body = get("/users-paginated?page=1&sort=name").await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["name"], json!("Taylor"));
    assert_eq!(
        body["links"],
        json!({
            "first": "http://localhost/users-paginated?page=1&sort=name",
            "last": "http://localhost/users-paginated?page=2&sort=name",
            "prev": null,
            "next": "http://localhost/users-paginated?page=2&sort=name",
        })
    );
    assert_eq!(body["meta"]["current_page"], json!(1));
    assert_eq!(body["meta"]["last_page"], json!(2));
    assert_eq!(body["meta"]["per_page"], json!(1));
    assert_eq!(body["meta"]["total"], json!(2));
    assert_eq!(
        body["meta"]["path"],
        json!("http://localhost/users-paginated")
    );

    let body = get("/users-paginated?page=2").await;
    assert_eq!(body["data"][0]["name"], json!("Abigail"));
    assert_eq!(body["links"]["next"], json!(null));
    assert_eq!(body["meta"]["from"], json!(2));
}

#[tokio::test]
async fn dedicated_collections_merge_their_links_with_the_paginator() {
    let _app = app().await;
    seed().await;
    routes();

    let body = get("/users-collection").await;
    assert_eq!(body["data"][0]["name"], json!("Taylor"));
    assert_eq!(body["links"]["self"], json!("link-value"));
    assert_eq!(
        body["links"]["next"],
        json!("http://localhost/users-collection?page=2")
    );
    assert_eq!(body["meta"]["key"], json!("value"));
    assert_eq!(body["meta"]["total"], json!(2));
}

#[tokio::test]
async fn created_resources_may_respond_with_201() {
    let _app = app().await;
    routes();

    let response = send("/users?name=Jess&email=jess@laravel.com", "POST").await;
    assert_eq!(response.status_code(), 201);
    let body = response.json_body();
    assert_eq!(body["data"]["name"], json!("Jess"));
    assert_eq!(body["data"]["id"], json!(1));
}

#[tokio::test]
async fn the_response_may_be_customized() {
    let _app = app().await;
    let (taylor, _) = seed().await;
    routes();

    let response = send(&format!("/headers/{}", taylor.id), "GET").await;
    assert_eq!(response.header("x-value").unwrap(), "True");
    assert_eq!(response.json_body()["data"]["name"], json!("Taylor"));
}

#[tokio::test]
async fn wrapping_may_be_disabled_for_the_application() {
    let _app = app().await;
    let (taylor, _) = seed().await;
    routes();
    Resource::without_wrapping();

    let body = get(&format!("/users/{}", taylor.id)).await;
    assert_eq!(body["name"], json!("Taylor"));

    let body = get("/users-paginated").await;
    assert_eq!(body["data"][0]["name"], json!("Taylor"));
    assert_eq!(body["meta"]["total"], json!(2));
}

#[tokio::test]
async fn when_has_reports_present_attributes() {
    let _app = app().await;
    let (taylor, _) = seed().await;

    struct Attributes(User);

    impl JsonResource for Attributes {
        type Model = User;

        fn from_model(user: User) -> Self {
            Self(user)
        }

        fn model(&self) -> &User {
            &self.0
        }

        fn to_array(&self, _request: &Request) -> Value {
            json!({
                "password": self.when_has("password"),
                "posts_count": self.when_has("posts_count"),
                "unknown": self.when_has("unknown"),
                "nothing": self.when_appended("name"),
            })
        }
    }

    let resolved = Attributes::make(&taylor).resolve(&Request::default());
    assert_eq!(
        resolved["password"].as_str().map(|p| !p.is_empty()),
        Some(true)
    );
    assert!(resolved.get("posts_count").is_none());
    assert!(resolved.get("unknown").is_none());
    assert!(resolved.get("nothing").is_none());

    let counted = User::with_count("posts")
        .find_or_fail(taylor.id)
        .await
        .unwrap();
    let resolved = Attributes::make(counted).resolve(&Request::default());
    assert_eq!(resolved["posts_count"], json!(2));
}
