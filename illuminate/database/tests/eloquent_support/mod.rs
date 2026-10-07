//! Models, factories and an in-memory database shared by the Eloquent tests.

#![allow(dead_code)]

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_database::eloquent::*;
use illuminate_database::{DatabaseManager, DatabaseServiceProvider, SchemaBuilder};

/// A fresh container with an in-memory SQLite database and every table
/// migrated. Keep the guard alive for the duration of the test.
pub async fn app() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({"database": {
        "default": "sqlite",
        "connections": {
            "sqlite": {"driver": "sqlite", "database": ":memory:", "foreign_key_constraints": false},
        },
    }})));
    DatabaseServiceProvider.register(&container);
    migrate(&schema()).await;
    (container, guard)
}

pub fn schema() -> SchemaBuilder {
    DatabaseManager::resolve()
        .default_connection()
        .get_schema_builder()
}

async fn migrate(schema: &SchemaBuilder) {
    schema
        .create("countries", |table| {
            table.id();
            table.string("name");
        })
        .await
        .unwrap();
    schema
        .create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.string("password").default("");
            table.boolean("active").default(true);
            table.json("options").nullable();
            table.foreign_id("country_id").nullable();
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("posts", |table| {
            table.id();
            table.foreign_id("user_id");
            table.string("title");
            table.text("body").nullable();
            table.boolean("published").default(false);
            table.integer("votes").default(0);
            table.timestamps();
            table.soft_deletes();
        })
        .await
        .unwrap();
    schema
        .create("comments", |table| {
            table.id();
            table.foreign_id("post_id");
            table.string("body");
            table.boolean("approved").default(false);
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("profiles", |table| {
            table.id();
            table.foreign_id("user_id");
            table.string("bio").nullable();
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
            table.boolean("active").default(true);
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("images", |table| {
            table.id();
            table.string("url");
            table.morphs("imageable");
        })
        .await
        .unwrap();
    schema
        .create("categories", |table| {
            table.id();
            table.foreign_id("parent_id").nullable();
            table.string("name");
        })
        .await
        .unwrap();
    schema
        .create("tickets", |table| {
            table.uuid("id").primary();
            table.string("title");
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("orders", |table| {
            table.ulid("id").primary();
            table.integer("total");
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("settings", |table| {
            table.id();
            table.string("key");
            table.string("value").nullable();
        })
        .await
        .unwrap();
    schema
        .create("flights", |table| {
            table.id();
            table.string("name");
        })
        .await
        .unwrap();
}

// ----------------------------------------------------------------------
// Models
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Default, Model)]
#[fillable(name, email, password, active, options, country_id)]
#[hidden(password)]
#[use_factory(UserFactory)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
    #[hashed]
    pub password: String,
    pub active: bool,
    pub options: Option<Vec<String>>,
    pub country_id: Option<i64>,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,

    #[computed]
    pub posts_count: Option<i64>,
    #[computed]
    pub pivot: Option<Value>,

    #[relation]
    pub posts: Option<Vec<Post>>,
    #[relation]
    pub profile: Option<Profile>,
    #[relation]
    pub roles: Option<Vec<Role>>,
    #[relation]
    pub country: Option<Country>,
    #[relation]
    pub image: Option<Image>,
}

impl User {
    pub fn posts(&self) -> HasMany<Self, Post> {
        self.has_many()
    }

    pub fn profile(&self) -> HasOne<Self, Profile> {
        self.has_one()
    }

    pub fn roles(&self) -> BelongsToMany<Self, Role> {
        self.belongs_to_many()
            .with_pivot(["active"])
            .with_timestamps()
    }

    pub fn country(&self) -> BelongsTo<Self, Country> {
        self.belongs_to()
    }

    pub fn image(&self) -> MorphOne<Self, Image> {
        self.morph_one("imageable")
    }

    pub fn active(query: Builder<Self>) -> Builder<Self> {
        query.where_("active", true)
    }
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(user_id, title, body, published, votes)]
#[soft_deletes]
#[use_factory(PostFactory)]
pub struct Post {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub body: Option<String>,
    pub published: bool,
    pub votes: i64,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
    pub deleted_at: Option<Carbon>,

    #[computed]
    pub comments_count: Option<i64>,

    #[relation]
    pub user: Option<Box<User>>,
    #[relation]
    pub comments: Option<Vec<Comment>>,
    #[relation]
    pub approved_comments: Vec<Comment>,
    #[relation]
    pub image: Option<Image>,
}

impl Post {
    pub fn user(&self) -> BelongsTo<Self, User> {
        self.belongs_to()
    }

    pub fn comments(&self) -> HasMany<Self, Comment> {
        self.has_many()
    }

    pub fn approved_comments(&self) -> HasMany<Self, Comment> {
        self.has_many().where_("approved", true)
    }

    pub fn image(&self) -> MorphOne<Self, Image> {
        self.morph_one("imageable")
    }
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(post_id, body, approved)]
pub struct Comment {
    pub id: i64,
    pub post_id: i64,
    pub body: String,
    pub approved: bool,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,

    #[relation]
    pub post: Option<Box<Post>>,
}

impl Comment {
    pub fn post(&self) -> BelongsTo<Self, Post> {
        self.belongs_to()
    }
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(user_id, bio)]
pub struct Profile {
    pub id: i64,
    pub user_id: i64,
    pub bio: Option<String>,
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(name)]
pub struct Role {
    pub id: i64,
    pub name: String,

    #[computed]
    pub pivot: Option<Value>,

    #[relation]
    pub users: Option<Vec<User>>,
}

impl Role {
    pub fn users(&self) -> BelongsToMany<Self, User> {
        self.belongs_to_many()
    }
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(name)]
pub struct Country {
    pub id: i64,
    pub name: String,

    #[relation]
    pub users: Option<Vec<User>>,
    #[relation]
    pub posts: Option<Vec<Post>>,
    #[relation]
    pub latest_post: Option<Box<Post>>,
}

impl Country {
    pub fn users(&self) -> HasMany<Self, User> {
        self.has_many()
    }

    pub fn posts(&self) -> HasManyThrough<Self, Post, User> {
        self.has_many_through()
    }

    pub fn latest_post(&self) -> HasOneThrough<Self, Post, User> {
        self.has_one_through().latest_by("posts.id")
    }
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(url, imageable_id, imageable_type)]
pub struct Image {
    pub id: i64,
    pub url: String,
    pub imageable_id: i64,
    pub imageable_type: String,

    #[relation]
    pub imageable_user: Option<Box<User>>,
    #[relation]
    pub imageable_post: Option<Box<Post>>,
}

impl Image {
    pub fn imageable_user(&self) -> MorphTo<Self, User> {
        self.morph_to("imageable")
    }

    pub fn imageable_post(&self) -> MorphTo<Self, Post> {
        self.morph_to("imageable")
    }
}

#[derive(Debug, Clone, Default, Model)]
#[table("categories")]
#[fillable(parent_id, name)]
pub struct Category {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,

    #[relation]
    pub parent: Option<Box<Category>>,
    #[relation]
    pub children: Option<Vec<Category>>,
}

impl Category {
    pub fn parent(&self) -> BelongsTo<Self, Category> {
        self.belongs_to().foreign_key("parent_id")
    }

    pub fn children(&self) -> HasMany<Self, Category> {
        self.has_many().foreign_key("parent_id")
    }
}

#[derive(Debug, Clone, Default, Model)]
#[has_uuids]
#[fillable(title)]
pub struct Ticket {
    pub id: String,
    pub title: String,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
}

#[derive(Debug, Clone, Default, Model)]
#[has_ulids]
#[fillable(total)]
#[route_key("id")]
pub struct Order {
    pub id: String,
    pub total: i64,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
}

#[derive(Debug, Clone, Default, Model)]
#[unguarded]
#[without_timestamps]
pub struct Setting {
    pub id: i64,
    pub key: String,
    pub value: Option<String>,
}

/// A totally guarded model.
#[derive(Debug, Clone, Default, Model)]
pub struct Flight {
    pub id: i64,
    pub name: String,
}

/// The `users` table, with only some attributes visible and an appended
/// accessor.
#[derive(Debug, Clone, Default, Model)]
#[table("users")]
#[visible(id, name, display_name)]
#[appends(display_name)]
#[route_key("email")]
pub struct Member {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
}

impl Member {
    pub fn display_name(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }
}

// ----------------------------------------------------------------------
// Factories
// ----------------------------------------------------------------------

#[derive(Default)]
pub struct UserFactory;

impl Factory for UserFactory {
    type Model = User;

    fn definition(&self, faker: &mut Faker) -> Value {
        json!({
            "name": faker.name(),
            "email": faker.unique(|faker| faker.safe_email()),
            "password": "password",
            "active": true,
        })
    }
}

pub trait UserFactoryStates {
    fn inactive(self) -> Self;
}

impl UserFactoryStates for FactoryBuilder<UserFactory> {
    fn inactive(self) -> Self {
        self.state(json!({"active": false}))
    }
}

#[derive(Default)]
pub struct PostFactory;

impl Factory for PostFactory {
    type Model = Post;

    fn definition(&self, faker: &mut Faker) -> Value {
        json!({
            "title": faker.sentence(),
            "body": faker.paragraph(),
            "published": faker.boolean(50),
        })
    }
}

// ----------------------------------------------------------------------
// Fixtures
// ----------------------------------------------------------------------

pub async fn user(name: &str) -> User {
    User::create(json!({
        "name": name,
        "email": format!("{}@laravel.com", name.to_lowercase()),
        "active": true,
    }))
    .await
    .unwrap()
}

pub async fn post(user: &User, title: &str) -> Post {
    user.posts()
        .create(json!({"title": title, "published": true}))
        .await
        .unwrap()
}
