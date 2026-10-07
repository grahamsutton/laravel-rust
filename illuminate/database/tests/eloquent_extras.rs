//! Eloquent's newer builder, model, relationship and collection features
//! against a live, in-memory SQLite database.

mod eloquent_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use eloquent_support::*;
use futures::StreamExt;
use illuminate_database::DB;
use illuminate_database::eloquent::*;
use illuminate_database::pagination::Cursor;
use illuminate_support::Result;
use illuminate_support::error::RuntimeException;

/// The `users` table, with dirty tracking (so instance visibility works).
#[derive(Debug, Clone, Default, Model)]
#[table("users")]
#[fillable(name, email)]
#[hidden(password, email)]
pub struct Writer {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub password: String,
    pub active: bool,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,

    #[computed]
    pub posts_sum_votes: Option<f64>,
    #[computed]
    pub posts_max_votes: Option<i64>,
    #[computed]
    pub posts_exists: Option<bool>,

    #[relation]
    pub posts: Option<Vec<Post>>,
    #[relation]
    pub teams: Option<Vec<Team>>,

    pub original: Original,
}

impl Writer {
    pub fn posts(&self) -> HasMany<Self, Post> {
        self.has_many().foreign_key("user_id")
    }

    pub fn latest_post(&self) -> HasOne<Self, Post> {
        self.posts().one().latest_by("posts.id")
    }

    pub fn teams(&self) -> BelongsToMany<Self, Team> {
        self.belongs_to_many()
            .table("role_user")
            .foreign_pivot_key("user_id")
            .related_pivot_key("role_id")
            .with_pivot(["active"])
            .as_("subscription")
    }
}

/// The `roles` table, exposing its pivot as `subscription`.
#[derive(Debug, Clone, Default, Model)]
#[table("roles")]
#[fillable(name)]
pub struct Team {
    pub id: i64,
    pub name: String,
    #[computed]
    pub subscription: Option<Value>,
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(name)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    #[computed]
    pub pivot: Option<Value>,

    #[relation]
    pub articles: Option<Vec<Article>>,
}

impl Tag {
    pub fn articles(&self) -> MorphToMany<Self, Article> {
        self.morphed_by_many("taggable")
    }
}

/// The `posts` table with tags.
#[derive(Debug, Clone, Default, Model)]
#[table("posts")]
#[fillable(user_id, title)]
#[soft_deletes]
pub struct Article {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub votes: i64,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
    pub deleted_at: Option<Carbon>,

    #[relation]
    pub tags: Option<Vec<Tag>>,
    #[relation]
    pub user: Option<Box<User>>,
}

impl Article {
    pub fn tags(&self) -> MorphToMany<Self, Tag> {
        self.morph_to_many("taggable")
    }

    pub fn user(&self) -> BelongsTo<Self, User> {
        self.belongs_to()
    }
}

async fn tag_tables() {
    schema()
        .create("tags", |table| {
            table.id();
            table.string("name").unique();
        })
        .await
        .unwrap();
    schema()
        .create("taggables", |table| {
            table.foreign_id("tag_id");
            table.morphs("taggable");
        })
        .await
        .unwrap();
}

async fn blog() -> Result<(User, User)> {
    let taylor = user("Taylor").await;
    let abigail = user("Abigail").await;
    let first = post(&taylor, "First").await;
    let second = post(&taylor, "Second").await;
    let third = post(&abigail, "Third").await;
    Post::query()
        .where_key(first.id)
        .update(json!({"votes": 10}))
        .await?;
    Post::query()
        .where_key(second.id)
        .update(json!({"votes": 5}))
        .await?;
    Post::query()
        .where_key(third.id)
        .update(json!({"votes": 1}))
        .await?;
    first
        .comments()
        .create(json!({"body": "Great", "approved": true}))
        .await?;
    first.comments().create(json!({"body": "Spam"})).await?;
    Ok((taylor, abigail))
}

fn titles(posts: &[Post]) -> Vec<&str> {
    posts.iter().map(|post| post.title.as_str()).collect()
}

// ----------------------------------------------------------------------
// The builder
// ----------------------------------------------------------------------

#[tokio::test]
async fn key_constraints_attributes_and_scopes() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;

    assert_eq!(
        Post::query().where_key(1).or_where_key(3).to_sql(),
        "select * from \"posts\" where (\"posts\".\"id\" = ? or \"posts\".\"id\" = ?) and \"posts\".\"deleted_at\" is null"
    );
    assert_eq!(
        User::query()
            .where_("active", true)
            .or_where_key_not(json!([1, 2]))
            .to_sql(),
        "select * from \"users\" where \"active\" = ? or \"users\".\"id\" not in (?, ?)"
    );

    let drafts = Post::query().with_attributes(json!({"published": false}));
    assert_eq!(
        drafts.to_sql(),
        "select * from \"posts\" where \"posts\".\"published\" = ? and \"posts\".\"deleted_at\" is null"
    );
    let draft = drafts
        .create(json!({"title": "Draft", "user_id": 1}))
        .await?;
    assert!(!draft.published);
    let published = Post::query()
        .with_attributes_as(json!({"published": true}), false)
        .make(json!({"title": "x"}))?;
    assert!(published.published);

    User::add_global_scope("active", |query| query.where_("active", true));
    User::add_global_scope("named", |query| query.where_not_null("name"));
    assert_eq!(
        User::query().without_global_scopes_except("named").to_sql(),
        "select * from \"users\" where \"name\" is not null"
    );
    assert_eq!(
        Flight::query()
            .with_global_scope("short", |query| query.where_("name", "A"))
            .to_sql(),
        "select * from \"flights\" where \"name\" = ?"
    );
    assert_eq!(
        Flight::query()
            .with_global_scope("short", |query| query.where_("name", "A"))
            .without_global_scope("short")
            .to_sql(),
        "select * from \"flights\""
    );

    let loads = Post::query()
        .with(["user", "comments"])
        .without_eager_load("user");
    assert_eq!(loads.get_eager_loads().len(), 1);
    assert!(loads.without_eager_loads().get_eager_loads().is_empty());
    Ok(())
}

#[tokio::test]
async fn or_callbacks_sole_lookups_and_after_query() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;

    let found = Post::query()
        .find_or(1, || async {
            Err::<Post, _>(RuntimeException::new("missing"))
        })
        .await?;
    assert_eq!(found.title, "First");
    let fallback = Post::query()
        .find_or(99, || async { Ok::<_, RuntimeException>(Post::default()) })
        .await?;
    assert_eq!(fallback.id, 0);
    let error = Post::query()
        .where_("title", "Nope")
        .first_or(|| async { Err::<Post, _>(RuntimeException::new("No posts!")) })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "No posts!");
    let fallback =
        Post::find_or(42, || async { Ok::<_, RuntimeException>(Post::default()) }).await?;
    assert_eq!(fallback.id, 0);

    assert_eq!(Post::query().find_sole(2).await?.title, "Second");
    assert!(
        Post::query()
            .find_sole(99)
            .await
            .unwrap_err()
            .downcast_ref::<ModelNotFoundException>()
            .is_some()
    );

    assert_eq!(
        Post::query().where_key(3).value_or_fail("title").await?,
        json!("Third")
    );
    let error = Post::query()
        .where_key(99)
        .value_or_fail("title")
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "No query results for model [Post].");

    let upper = Post::query()
        .after_query(|posts| {
            posts
                .into_iter()
                .map(|mut post| {
                    post.title = post.title.to_uppercase();
                    post
                })
                .collect()
        })
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(titles(&upper), ["FIRST", "SECOND", "THIRD"]);
    Ok(())
}

#[tokio::test]
async fn create_or_first_increment_or_create_and_quiet_creates() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;

    let same = User::query()
        .create_or_first(
            json!({"email": "taylor@laravel.com"}),
            json!({"name": "Other"}),
        )
        .await?;
    assert_eq!(same.id, taylor.id);
    assert_eq!(same.name, "Taylor");
    let created = User::query()
        .create_or_first(
            json!({"email": "nuno@laravel.com"}),
            json!({"name": "Nuno"}),
        )
        .await?;
    assert!(created.id > taylor.id);
    assert_eq!(User::count().await?, 2);

    let counter = Post::query()
        .increment_or_create(json!({"title": "Counter", "user_id": 1}), "votes", 1, 1)
        .await?;
    assert_eq!(counter.votes, 1);
    let counter = Post::query()
        .increment_or_create(json!({"title": "Counter", "user_id": 1}), "votes", 1, 5)
        .await?;
    assert_eq!(counter.votes, 6);
    assert_eq!(Post::find_or_fail(counter.id).await?.votes, 6);

    let fired = Arc::new(AtomicUsize::new(0));
    let seen = fired.clone();
    User::created(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    User::create_quietly(json!({"name": "Q", "email": "q@laravel.com"})).await?;
    User::query()
        .force_create_quietly(json!({"name": "F", "email": "f@laravel.com"}))
        .await?;
    User::force_create_quietly(json!({"name": "G", "email": "g@laravel.com"})).await?;
    assert_eq!(fired.load(Ordering::SeqCst), 0);
    User::create(json!({"name": "L", "email": "l@laravel.com"})).await?;
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn cursors_lazy_streams_and_cursor_pagination() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;

    let streamed: Vec<String> = Post::query()
        .order_by("id", "asc")
        .cursor()
        .map(|post| post.unwrap().title)
        .collect()
        .await;
    assert_eq!(streamed, ["First", "Second", "Third"]);

    let lazy: Vec<i64> = Post::query()
        .lazy(2)
        .map(|post| post.unwrap().id)
        .collect()
        .await;
    assert_eq!(lazy, [1, 2, 3]);
    let lazy: Vec<i64> = Post::query()
        .lazy_by_id_desc(2)
        .map(|post| post.unwrap().id)
        .collect()
        .await;
    assert_eq!(lazy, [3, 2, 1]);
    let lazy: Vec<i64> = Post::query()
        .lazy_by_id(1)
        .map(|post| post.unwrap().id)
        .collect()
        .await;
    assert_eq!(lazy, [1, 2, 3]);

    let mut seen = Vec::new();
    Post::query()
        .each_by_id(2, |post| {
            seen.push(post.id);
            async { Ok(true) }
        })
        .await?;
    assert_eq!(seen, [1, 2, 3]);
    let lengths = Post::query()
        .chunk_map(|post| async move { Ok(post.title.len()) }, 2)
        .await?;
    assert_eq!(lengths.all(), &[5, 6, 5]);

    let page = Post::query()
        .order_by("votes", "desc")
        .cursor_paginate(2, None)
        .await?;
    assert_eq!(titles(page.items()), ["First", "Second"]);
    let next = page.next_cursor().unwrap();
    assert_eq!(next.parameter("votes")?, json!(5));
    let page = Post::query()
        .order_by("votes", "desc")
        .cursor_paginate(2, Some(next))
        .await?;
    assert_eq!(titles(page.items()), ["Third"]);
    let back = Post::query()
        .order_by("votes", "desc")
        .cursor_paginate(2, page.previous_cursor())
        .await?;
    assert_eq!(titles(back.items()), ["First", "Second"]);
    let page = Post::query()
        .order_by("id", "asc")
        .cursor_paginate(None, Some(Cursor::new(json!({"id": 1}), true)))
        .await?;
    assert_eq!(page.count(), 2);
    Ok(())
}

// ----------------------------------------------------------------------
// Models
// ----------------------------------------------------------------------

#[tokio::test]
async fn transactional_and_quiet_persistence() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, _) = blog().await?;

    let saved = Arc::new(AtomicUsize::new(0));
    let seen = saved.clone();
    Writer::saved(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });

    let mut writer = Writer::find_or_fail(taylor.id).await?;
    writer.name = "Otwell".into();
    assert!(writer.save_or_fail().await?);
    assert!(
        writer
            .update_or_fail(json!({"name": "Taylor Otwell"}))
            .await?
    );
    assert_eq!(saved.load(Ordering::SeqCst), 2);
    assert!(writer.update_quietly(json!({"name": "Quiet"})).await?);
    assert_eq!(saved.load(Ordering::SeqCst), 2);
    assert_eq!(Writer::find_or_fail(taylor.id).await?.name, "Quiet");

    let before = writer.updated_at;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(writer.touch_quietly().await?);
    assert_ne!(writer.updated_at, before);
    assert_eq!(saved.load(Ordering::SeqCst), 2);

    // A failing listener rolls the transaction back.
    Writer::saving(|writer: &mut Writer| -> Result<bool> {
        if writer.name == "Boom" {
            return Err(RuntimeException::new("boom").into());
        }
        Ok(true)
    });
    writer.name = "Boom".into();
    assert!(writer.save_or_fail().await.is_err());
    assert_eq!(Writer::find_or_fail(taylor.id).await?.name, "Quiet");

    let mut fresh = Writer {
        name: "Ignored".into(),
        email: "taylor@laravel.com".into(),
        ..Default::default()
    };
    assert!(!fresh.save_or_ignore(None).await?);
    assert!(!fresh.exists());
    let mut fresh = Writer {
        name: "Nuno".into(),
        email: "nuno@laravel.com".into(),
        ..Default::default()
    };
    assert!(fresh.save_or_ignore(Some(&["email"])).await?);
    assert!(fresh.exists() && fresh.id > 0);
    let error = fresh.save_or_ignore(None).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot use saveOrIgnore on an existing model."
    );

    let mut doomed = Writer::find_or_fail(fresh.id).await?;
    assert!(doomed.delete_or_fail().await?);
    assert!(Writer::find(fresh.id).await?.is_none());
    assert!(!doomed.delete_or_fail().await?);
    Ok(())
}

#[tokio::test]
async fn push_replicate_refresh_and_timestamps() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, _) = blog().await?;

    let mut writer = Writer::query()
        .with("posts")
        .find_or_fail(taylor.id)
        .await?;
    writer.name = "Pushed".into();
    writer.posts.as_mut().unwrap()[0].title = "Pushed post".into();
    assert!(writer.push().await?);
    assert_eq!(Writer::find_or_fail(taylor.id).await?.name, "Pushed");
    assert_eq!(Post::find_or_fail(1).await?.title, "Pushed post");

    let created = Arc::new(AtomicUsize::new(0));
    let seen = created.clone();
    Post::saved(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    writer.posts.as_mut().unwrap()[1].title = "Quietly".into();
    assert!(writer.push_quietly().await?);
    assert_eq!(created.load(Ordering::SeqCst), 0);
    assert_eq!(Post::find_or_fail(2).await?.title, "Quietly");

    let replicating = Arc::new(AtomicUsize::new(0));
    let seen = replicating.clone();
    Post::replicating(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    let copy = Post::find_or_fail(1).await?.replicate_quietly().await?;
    assert_eq!(copy.id, 0);
    assert_eq!(copy.title, "Pushed post");
    assert_eq!(replicating.load(Ordering::SeqCst), 0);

    let mut stale = Writer::find_or_fail(taylor.id).await?;
    User::query()
        .where_key(taylor.id)
        .update(json!({"name": "Changed"}))
        .await?;
    stale.refresh_for_update().await?;
    assert_eq!(stale.name, "Changed");

    let mut writer = Writer::find_or_fail(taylor.id).await?;
    let stamp = writer.updated_at;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(!Writer::is_ignoring_timestamps());
    Writer::without_timestamps(async {
        assert!(Writer::is_ignoring_timestamps());
        assert!(!User::is_ignoring_timestamps());
        writer.name = "No stamp".into();
        writer.save().await
    })
    .await?;
    assert_eq!(writer.updated_at, stamp);
    assert_eq!(Writer::find_or_fail(taylor.id).await?.updated_at, stamp);
    Ok(())
}

#[tokio::test]
async fn soft_delete_helpers() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;

    let deleted = Arc::new(AtomicUsize::new(0));
    let seen = deleted.clone();
    Post::deleted(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });

    let mut first = Post::find_or_fail(1).await?;
    first.delete().await?;
    assert_eq!(deleted.load(Ordering::SeqCst), 1);
    assert!(first.restore_quietly().await?);
    assert!(Post::find(1).await?.is_some());

    assert!(first.force_delete_quietly().await?);
    assert_eq!(deleted.load(Ordering::SeqCst), 1);
    assert!(Post::with_trashed().find(1).await?.is_none());

    Post::find_or_fail(2).await?.delete().await?;
    assert_eq!(Post::force_destroy(vec![2, 3]).await?, 2);
    assert_eq!(Post::with_trashed().count().await?, 0);
    Ok(())
}

#[tokio::test]
async fn strict_mode_switches() -> Result<()> {
    let (_app, _guard) = app().await;
    assert!(!prevents_silently_discarding_attributes());
    let mut post = Post::default();
    post.fill(json!({"title": "Fine", "secret": "dropped"}))?;
    assert_eq!(post.title, "Fine");

    should_be_strict(true);
    assert!(prevents_lazy_loading());
    assert!(prevents_accessing_missing_attributes());
    let error = Post::default()
        .fill(json!({"title": "x", "secret": 1, "other": 2}))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Add [secret, other] to fillable property to allow mass assignment on [Post]."
    );
    assert!(error.downcast_ref::<MassAssignmentException>().is_some());

    let discarded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = discarded.clone();
    handle_discarded_attribute_violation_using(move |class, keys| {
        seen.lock()
            .unwrap()
            .push(format!("{class}: {}", keys.join(",")));
    });
    Post::default().fill(json!({"title": "x", "secret": 1}))?;
    assert_eq!(discarded.lock().unwrap().as_slice(), ["Post: secret"]);

    user("Taylor").await;
    let partial = Writer::query()
        .select(["id", "name"])
        .first_or_fail()
        .await?;
    let error = partial.try_get_attribute("email").unwrap_err();
    assert_eq!(
        error.to_string(),
        "The attribute [email] either does not exist or was not retrieved for model [Writer]."
    );
    assert_eq!(partial.try_get_attribute("name")?, json!("Taylor"));
    assert!(partial.try_get_attribute("nope").is_err());
    let missing = Arc::new(AtomicUsize::new(0));
    let seen = missing.clone();
    handle_missing_attribute_violation_using(move |_, _| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(partial.try_get_attribute("email")?, Value::Null);
    assert_eq!(missing.load(Ordering::SeqCst), 1);

    should_be_strict(false);
    assert!(!prevents_lazy_loading());
    assert_eq!(partial.try_get_attribute("email")?, json!(""));
    Ok(())
}

#[tokio::test]
async fn visibility_relations_and_aggregates_on_models() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, _) = blog().await?;

    let mut writer = Writer::find_or_fail(taylor.id).await?;
    assert!(!writer.to_array().contains_key("email"));
    writer.make_visible("email");
    assert_eq!(writer.to_array()["email"], json!("taylor@laravel.com"));
    writer.make_hidden(["name", "active"]);
    let array = writer.to_array();
    assert!(!array.contains_key("name") && !array.contains_key("active"));
    writer
        .make_visible_if(false, "name")
        .make_hidden_if(true, "id");
    assert!(!writer.to_array().contains_key("id"));
    assert_eq!(writer.get_hidden(), ["password", "name", "active", "id"]);
    // A fresh instance keeps the class defaults.
    assert!(
        !Writer::find_or_fail(taylor.id)
            .await?
            .to_array()
            .contains_key("email")
    );

    writer.load("posts").await?;
    assert!(writer.relation_loaded("posts"));
    assert!(!writer.relation_loaded("teams"));
    let bare = writer.without_relations();
    assert!(!bare.relation_loaded("posts"));
    assert!(writer.relation_loaded("posts"));
    assert!(!writer.without_relation("posts").relation_loaded("posts"));

    writer.load_sum("posts", "votes").await?;
    writer.load_max("posts", "votes").await?;
    writer.load_exists("posts").await?;
    assert_eq!(writer.posts_sum_votes, Some(15.0));
    assert_eq!(writer.posts_max_votes, Some(10));
    assert_eq!(writer.posts_exists, Some(true));

    let mut post = Post::find_or_fail(1).await?;
    post.load_count("comments").await?;
    assert_eq!(post.comments_count, Some(2));

    let latest = writer.latest_post().get().await?.unwrap();
    assert_eq!(latest.title, "Second");
    Ok(())
}

#[tokio::test]
async fn morph_loading() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, _) = blog().await?;
    let first = Post::find_or_fail(1).await?;
    first.image().create(json!({"url": "post.png"})).await?;
    taylor.image().create(json!({"url": "taylor.png"})).await?;

    let mut image = Image::query()
        .where_("url", "post.png")
        .first_or_fail()
        .await?;
    image.load_morph("imageable_post", "comments").await?;
    let post = image.imageable_post.as_ref().unwrap();
    assert_eq!(post.comments.as_ref().unwrap().len(), 2);

    let mut images = Image::all().await?;
    images
        .load_morph_count("imageable_post", "comments")
        .await?;
    let post_image = images.iter().find(|image| image.url == "post.png").unwrap();
    assert_eq!(
        post_image.imageable_post.as_ref().unwrap().comments_count,
        Some(2)
    );
    assert!(
        images
            .iter()
            .find(|image| image.url == "taylor.png")
            .unwrap()
            .imageable_post
            .is_none()
    );

    let mut image = Image::query()
        .where_("url", "post.png")
        .first_or_fail()
        .await?;
    image.load_morph_count("imageable_post", "comments").await?;
    assert_eq!(image.imageable_post.unwrap().comments_count, Some(2));

    let with = image_post_with_comments().await?;
    assert_eq!(with.comments.unwrap().len(), 2);
    Ok(())
}

async fn image_post_with_comments() -> Result<Post> {
    let image = Image::query()
        .where_("url", "post.png")
        .first_or_fail()
        .await?;
    Ok(image
        .imageable_post()
        .morph_with("comments")
        .first()
        .await?
        .unwrap())
}

// ----------------------------------------------------------------------
// Relationship queries
// ----------------------------------------------------------------------

#[tokio::test]
async fn where_belongs_to_and_where_attached_to() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, abigail) = blog().await?;

    let posts = Post::query().where_belongs_to(&taylor).get().await?;
    assert_eq!(titles(&posts), ["First", "Second"]);
    assert_eq!(
        Post::query().where_belongs_to(&taylor).to_sql(),
        "select * from \"posts\" where \"posts\".\"user_id\" = ? and \"posts\".\"deleted_at\" is null"
    );
    let both = Post::query()
        .where_belongs_to(vec![taylor.clone(), abigail.clone()])
        .count()
        .await?;
    assert_eq!(both, 3);
    let either = Post::query()
        .where_("title", "Nope")
        .or_where_belongs_to_relation(&abigail, "user")
        .get()
        .await?;
    assert_eq!(titles(&either), ["Third"]);
    let error = Post::query()
        .where_belongs_to_relation(&taylor, "comments")
        .get()
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Call to undefined relationship [comments] on model [Post]."
    );

    let admin = Role::create(json!({"name": "admin"})).await?;
    let editor = Role::create(json!({"name": "editor"})).await?;
    taylor.roles().attach([admin.id]).await?;
    abigail.roles().attach([editor.id]).await?;
    let admins = User::query()
        .where_attached_to(&admin)
        .pluck("name")
        .await?;
    assert_eq!(admins.all(), &[json!("Taylor")]);
    let staff = User::query()
        .where_attached_to(vec![admin.clone(), editor.clone()])
        .count()
        .await?;
    assert_eq!(staff, 2);
    let staff = User::query()
        .where_("name", "Nobody")
        .or_where_attached_to_relation(&editor, "roles")
        .pluck("name")
        .await?;
    assert_eq!(staff.all(), &[json!("Abigail")]);
    Ok(())
}

#[tokio::test]
async fn morph_existence_queries() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, abigail) = blog().await?;
    let first = Post::find_or_fail(1).await?;
    first.image().create(json!({"url": "post.png"})).await?;
    taylor.image().create(json!({"url": "taylor.png"})).await?;
    abigail
        .image()
        .create(json!({"url": "abigail.png"}))
        .await?;

    let urls = |images: Collection<Image>| -> Vec<String> {
        images.iter().map(|image| image.url.clone()).collect()
    };
    let posts_or_users = Image::query()
        .has_morph(["imageable_post", "imageable_user"])
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(posts_or_users.len(), 3);
    assert_eq!(
        Image::query()
            .has_morph(["imageable_post", "imageable_user"])
            .to_sql(),
        "select * from \"images\" where (exists (select * from \"posts\" where \"posts\".\"id\" = \"images\".\"imageable_id\" and \"images\".\"imageable_type\" = ? and \"posts\".\"deleted_at\" is null) or exists (select * from \"users\" where \"users\".\"id\" = \"images\".\"imageable_id\" and \"images\".\"imageable_type\" = ?))"
    );
    let posts_only = Image::query().has_morph("imageable_post").get().await?;
    assert_eq!(urls(posts_only), ["post.png"]);
    let not_posts = Image::query()
        .doesnt_have_morph("imageable_post")
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(urls(not_posts), ["taylor.png", "abigail.png"]);

    // The constraint runs against each relation's own table.
    let recent = Image::query()
        .where_has_morph(["imageable_post", "imageable_user"], |query| {
            query.where_op("id", ">", 1)
        })
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(urls(recent), ["abigail.png"]);

    let users = Image::query()
        .where_morph_relation("imageable_user", "name", "Taylor")
        .get()
        .await?;
    assert_eq!(urls(users), ["taylor.png"]);
    let users = Image::query()
        .where_morph_doesnt_have_relation("imageable_user", "name", "Taylor")
        .or_where_morph_relation("imageable_post", "title", "First")
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(urls(users), ["post.png", "abigail.png"]);
    let none = Image::query()
        .where_has_morph("imageable_user", |query| query.where_("name", "Nobody"))
        .or_where_doesnt_have_morph("imageable_post", |query| query.where_("title", "First"))
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(urls(none), ["taylor.png", "abigail.png"]);
    assert_eq!(
        Image::query()
            .where_("url", "x")
            .or_has_morph("imageable_post")
            .or_doesnt_have_morph("imageable_user")
            .or_where_has_morph("imageable_post", |query| query)
            .count()
            .await?,
        1
    );

    let morphed = Image::query()
        .where_morphed_to("imageable_user", &taylor)
        .get()
        .await?;
    assert_eq!(urls(morphed), ["taylor.png"]);
    assert_eq!(
        Image::query()
            .where_morphed_to("imageable_user", &taylor)
            .to_sql(),
        "select * from \"images\" where ((\"images\".\"imageable_type\" = ? and \"images\".\"imageable_id\" in (?)))"
    );
    let morphed = Image::query()
        .where_morphed_to("imageable_user", vec![taylor.clone(), abigail.clone()])
        .count()
        .await?;
    assert_eq!(morphed, 2);
    let others = Image::query()
        .where_not_morphed_to("imageable_user", &taylor)
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(urls(others), ["post.png", "abigail.png"]);
    let either = Image::query()
        .where_morphed_to("imageable_post", &first)
        .or_where_morphed_to("imageable_user", &abigail)
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(urls(either), ["post.png", "abigail.png"]);
    let error = Image::query()
        .where_morphed_to("imageable_user", &taylor)
        .or_where_not_morphed_to("missing", &taylor)
        .get()
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Call to undefined relationship [missing] on model [Image]."
    );
    Ok(())
}

#[tokio::test]
async fn relation_existence_shortcuts() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;

    let without = Post::query()
        .where_doesnt_have_relation("comments", "approved", true)
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(titles(&without), ["Second", "Third"]);
    let without = Post::query()
        .where_("title", "First")
        .or_where_doesnt_have_relation("comments", "body", "Spam")
        .count()
        .await?;
    assert_eq!(without, 3);
    let without = Post::query()
        .where_doesnt_have_relation_op("comments", "id", ">", 0)
        .count()
        .await?;
    assert_eq!(without, 2);

    let posts = Post::query()
        .with_where_has("comments", |query| query.where_("approved", true))
        .get()
        .await?;
    assert_eq!(titles(&posts), ["First"]);
    assert_eq!(posts[0].comments.as_ref().unwrap().len(), 1);
    let posts = Post::query()
        .with_where_relation("comments", "body", "Spam")
        .get()
        .await?;
    assert_eq!(posts[0].comments.as_ref().unwrap()[0].body, "Spam");
    let posts = Post::query()
        .where_("title", "Nope")
        .or_where_relation_op("comments", "id", ">=", 2)
        .get()
        .await?;
    assert_eq!(titles(&posts), ["First"]);
    Ok(())
}

// ----------------------------------------------------------------------
// Relations
// ----------------------------------------------------------------------

#[tokio::test]
async fn has_many_helpers() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;

    let made = taylor
        .posts()
        .make_many([json!({"title": "A"}), json!({"title": "B"})])?;
    assert_eq!(made.len(), 2);
    assert!(
        made.iter()
            .all(|post| post.user_id == taylor.id && post.id == 0)
    );

    let fired = Arc::new(AtomicUsize::new(0));
    let seen = fired.clone();
    Post::created(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    taylor
        .posts()
        .force_create_many([json!({"title": "C", "votes": 3})])
        .await?;
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    taylor.posts().create_quietly(json!({"title": "D"})).await?;
    taylor
        .posts()
        .force_create_quietly(json!({"title": "E"}))
        .await?;
    taylor
        .posts()
        .create_many_quietly([json!({"title": "F"})])
        .await?;
    taylor
        .posts()
        .force_create_many_quietly([json!({"title": "G"})])
        .await?;
    let mut extra = vec![Post {
        title: "H".into(),
        ..Default::default()
    }];
    taylor.posts().save_many_quietly(&mut extra).await?;
    let mut single = Post {
        title: "I".into(),
        ..Default::default()
    };
    taylor.posts().save_quietly(&mut single).await?;
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    assert_eq!(taylor.posts().count().await?, 7);

    let votes = taylor
        .posts()
        .increment_or_create(json!({"title": "C"}), "votes", 1, 2)
        .await?;
    assert_eq!(votes.votes, 5);
    let fresh = taylor
        .posts()
        .increment_or_create(json!({"title": "Z"}), "votes", 7, 2)
        .await?;
    assert_eq!((fresh.votes, fresh.user_id), (7, taylor.id));

    let existing = taylor
        .posts()
        .create_or_first(json!({"title": "C"}), json!({}))
        .await?;
    assert!(existing.id > 0);

    let latest = taylor
        .posts()
        .one()
        .latest_by("posts.id")
        .get()
        .await?
        .unwrap();
    assert_eq!(latest.title, "C");
    let all_ids: Vec<i64> = taylor
        .posts()
        .lazy(3)
        .map(|post| post.unwrap().id)
        .collect()
        .await;
    assert_eq!(all_ids.len(), 9);
    let first = taylor
        .posts()
        .find_or(1, || async { Err::<Post, _>(RuntimeException::new("x")) })
        .await?;
    assert_eq!(first.title, "C");
    assert!(taylor.posts().find_sole(999).await.is_err());
    let page = taylor
        .posts()
        .order_by("posts.id", "asc")
        .cursor_paginate(4, None)
        .await?;
    assert_eq!(page.count(), 4);
    let next = taylor
        .posts()
        .order_by("posts.id", "asc")
        .cursor_paginate(4, page.next_cursor())
        .await?;
    assert_eq!(next.items()[0].id, page.items()[3].id + 1);
    Ok(())
}

#[tokio::test]
async fn belongs_to_helpers_and_morph_maps() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, _) = blog().await?;

    let parent = Category::create(json!({"name": "Parent"})).await?;
    let mut child = Category::create(json!({"name": "Child", "parent_id": parent.id})).await?;
    child.parent().disassociate(&mut child)?;
    assert_eq!(child.parent_id, None);

    let all = no_constraints(async { taylor.posts().get().await }).await?;
    assert_eq!(all.len(), 3);
    assert!(!constraints_disabled());

    require_morph_map(true);
    assert!(requires_morph_map());
    let error = taylor
        .image()
        .create(json!({"url": "x.png"}))
        .await
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<ClassMorphViolationException>()
            .is_some()
    );
    assert_eq!(error.to_string(), "No morph map defined for model [User].");
    enforce_morph_map([("user", "User"), ("post", "Post")]);
    let image = taylor.image().create(json!({"url": "x.png"})).await?;
    assert_eq!(image.imageable_type, "user");
    require_morph_map(false);
    Ok(())
}

#[tokio::test]
async fn belongs_to_many_helpers() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, _) = blog().await?;
    let admin = Role::create(json!({"name": "admin"})).await?;
    let editor = Role::create(json!({"name": "editor"})).await?;
    let viewer = Role::create(json!({"name": "viewer"})).await?;
    taylor.roles().attach([admin.id]).await?;
    taylor
        .roles()
        .attach_with([editor.id], json!({"active": false}))
        .await?;
    taylor.roles().attach([viewer.id]).await?;

    let names = |roles: Collection<Role>| -> Vec<String> {
        roles.iter().map(|role| role.name.clone()).collect()
    };
    let roles = taylor
        .roles()
        .where_pivot("active", false)
        .or_where_pivot_in("role_id", [viewer.id])
        .order_by_pivot_desc("role_id")
        .get()
        .await?;
    assert_eq!(names(roles), ["viewer", "editor"]);
    assert_eq!(
        taylor
            .roles()
            .where_pivot_between("role_id", admin.id, editor.id)
            .count()
            .await?,
        2
    );
    assert_eq!(
        taylor
            .roles()
            .where_pivot_not_between("role_id", admin.id, editor.id)
            .or_where_pivot_null("created_at")
            .count()
            .await?,
        1
    );
    assert_eq!(
        taylor
            .roles()
            .where_pivot("role_id", admin.id)
            .or_where_pivot_not_in("role_id", [admin.id, editor.id])
            .or_where_pivot_not_null("nope_never")
            .to_sql(),
        "select * from \"roles\" inner join \"role_user\" on \"roles\".\"id\" = \"role_user\".\"role_id\" where (\"role_user\".\"role_id\" = ? or \"role_user\".\"role_id\" not in (?, ?) or \"role_user\".\"nope_never\" is not null) and \"role_user\".\"user_id\" = ?"
    );
    assert_eq!(
        taylor
            .roles()
            .order_by_pivot("role_id", "asc")
            .or_where_pivot_between("role_id", 1, 1)
            .get()
            .await?
            .len(),
        1
    );
    let ids = taylor.roles().all_related_ids().await?;
    assert_eq!(ids.len(), 3);

    let writer = Writer::find_or_fail(taylor.id).await?;
    let teams = writer.teams().order_by("roles.id", "asc").get().await?;
    assert_eq!(teams[0].subscription.as_ref().unwrap()["active"], json!(1));
    assert_eq!(writer.teams().get_pivot_accessor(), "subscription");

    let quiet = taylor
        .roles()
        .create_quietly(json!({"name": "quiet"}))
        .await?;
    let many = taylor
        .roles()
        .create_many_quietly([json!({"name": "a"}), json!({"name": "b"})])
        .await?;
    assert_eq!(many.len(), 2);
    let mut role = Role {
        name: "saved".into(),
        ..Default::default()
    };
    taylor.roles().save_quietly(&mut role).await?;
    let mut roles = vec![Role {
        name: "c".into(),
        ..Default::default()
    }];
    taylor.roles().save_many_quietly(&mut roles).await?;
    assert_eq!(taylor.roles().count().await?, 8);
    assert!(taylor.roles().find(quiet.id).await?.is_some());
    let first = taylor
        .roles()
        .first_or(|| async { Err::<Role, _>(RuntimeException::new("none")) })
        .await?;
    assert!(first.id > 0);

    let streamed: Vec<i64> = taylor
        .roles()
        .lazy_by_id(3)
        .map(|role| role.unwrap().id)
        .collect()
        .await;
    assert_eq!(streamed.len(), 8);
    let streamed: Vec<i64> = taylor
        .roles()
        .cursor()
        .map(|role| role.unwrap().id)
        .collect()
        .await;
    assert_eq!(streamed.len(), 8);
    let mut seen = 0;
    taylor
        .roles()
        .each_by_id(2, |_| {
            seen += 1;
            async { Ok(true) }
        })
        .await?;
    assert_eq!(seen, 8);
    let names = taylor
        .roles()
        .chunk_map(|role| async move { Ok(role.name) }, 5)
        .await?;
    assert_eq!(names.len(), 8);
    Ok(())
}

#[tokio::test]
async fn polymorphic_many_to_many() -> Result<()> {
    let (_app, _guard) = app().await;
    tag_tables().await;
    blog().await?;
    let article = Article::find_or_fail(1).await?;
    let other = Article::find_or_fail(2).await?;
    let rust = Tag::create(json!({"name": "rust"})).await?;
    let php = Tag::create(json!({"name": "php"})).await?;

    article.tags().attach([rust.id, php.id]).await?;
    other.tags().attach([rust.id]).await?;
    assert_eq!(
        DB::table("taggables")
            .where_("taggable_type", "Article")
            .count()
            .await?,
        3
    );
    assert_eq!(
        article.tags().to_sql(),
        "select * from \"tags\" inner join \"taggables\" on \"tags\".\"id\" = \"taggables\".\"tag_id\" where \"taggables\".\"taggable_type\" = ? and \"taggables\".\"taggable_id\" = ?"
    );
    let tags: Vec<String> = article
        .tags()
        .order_by("tags.id", "asc")
        .get()
        .await?
        .iter()
        .map(|tag| tag.name.clone())
        .collect();
    assert_eq!(tags, ["rust", "php"]);
    let articles: Vec<String> = rust
        .articles()
        .order_by("posts.id", "asc")
        .get()
        .await?
        .iter()
        .map(|article| article.title.clone())
        .collect();
    assert_eq!(articles, ["First", "Second"]);
    let loaded = Tag::with("articles").order_by("id", "asc").get().await?;
    assert_eq!(loaded[0].articles.as_ref().unwrap().len(), 2);

    let with = Article::with("tags").order_by("id", "asc").get().await?;
    assert_eq!(with[0].tags.as_ref().unwrap().len(), 2);
    assert_eq!(with[1].tags.as_ref().unwrap().len(), 1);
    assert!(with[2].tags.as_ref().unwrap().is_empty());
    assert_eq!(Article::query().has("tags").count().await?, 2);

    article.tags().detach([php.id]).await?;
    assert_eq!(article.tags().count().await?, 1);
    let synced = other.tags().sync([php.id]).await?;
    assert_eq!(synced.attached, [json!(php.id)]);
    assert_eq!(synced.detached, [json!(rust.id)]);
    Ok(())
}

// ----------------------------------------------------------------------
// Collections
// ----------------------------------------------------------------------

#[tokio::test]
async fn collection_aggregates_and_visibility() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;

    let mut writers = Writer::query().order_by("id", "asc").get().await?;
    writers.load_sum("posts", "votes").await?;
    writers.load_exists("posts").await?;
    assert_eq!(writers[0].posts_sum_votes, Some(15.0));
    assert_eq!(writers[1].posts_sum_votes, Some(1.0));
    assert_eq!(writers[0].posts_exists, Some(true));
    writers.load_max("posts", "votes").await?;
    assert_eq!(writers[1].posts_max_votes, Some(1));
    writers.load_min("posts", "votes").await?;
    writers.load_avg("posts", "votes").await?;

    writers.make_visible("email");
    assert!(
        writers
            .iter()
            .all(|writer| writer.to_array().contains_key("email"))
    );
    writers.make_hidden("email");
    assert!(
        writers
            .iter()
            .all(|writer| !writer.to_array().contains_key("email"))
    );

    writers.load("posts").await?;
    let bare = writers.without_relations();
    assert!(bare.iter().all(|writer| !writer.relation_loaded("posts")));

    let mut posts = Post::query().order_by("id", "asc").get().await?;
    posts.load_aggregate("comments", "*", "count").await?;
    assert_eq!(posts[0].comments_count, Some(2));
    Ok(())
}

#[tokio::test]
async fn through_relations_morph_counts_and_attribute_helpers() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, abigail) = blog().await?;
    let country = Country::create(json!({"name": "Netherlands"})).await?;
    User::query()
        .where_key(json!([taylor.id, abigail.id]))
        .update(json!({"country_id": country.id}))
        .await?;

    let latest = country.posts().one().latest_by("posts.id").get().await?;
    assert_eq!(latest.unwrap().title, "Third");
    assert!(!country.posts().through_parent_soft_deletes());
    assert_eq!(
        country.posts().with_trashed_parents().to_sql(),
        country.posts().to_sql()
    );
    let streamed: Vec<i64> = country
        .posts()
        .lazy_by_id(2)
        .map(|post| post.unwrap().id)
        .collect()
        .await;
    assert_eq!(streamed, [1, 2, 3]);

    let first = Post::find_or_fail(1).await?;
    first.image().create(json!({"url": "post.png"})).await?;
    let image = Image::query().first_or_fail().await?;
    let post = image
        .imageable_post()
        .morph_with_count("comments")
        .first()
        .await?
        .unwrap();
    assert_eq!(post.comments_count, Some(2));

    let writer = Writer::query()
        .with("posts")
        .find_or_fail(taylor.id)
        .await?;
    assert_eq!(
        Value::Object(writer.only(["name", "email"])),
        json!({"name": "Taylor", "email": "taylor@laravel.com"})
    );
    let except = writer.except(["password", "created_at", "updated_at"]);
    assert_eq!(
        except.keys().collect::<Vec<_>>(),
        ["id", "name", "email", "active"]
    );
    assert!(!writer.attributes_to_array().contains_key("posts"));
    assert!(writer.relations_to_array().contains_key("posts"));

    let mut fresh = Writer {
        name: "New".into(),
        ..Default::default()
    };
    fresh.update_timestamps()?;
    assert!(fresh.created_at.is_some() && fresh.updated_at.is_some());
    assert_eq!(Writer::fresh_timestamp_string().len(), 19);

    let nested = Post::query().where_("published", true).where_nested(
        |query| query.where_("votes", 1).or_where("votes", 10),
        "and",
    );
    assert_eq!(nested.count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn except_write_connections_and_joining_tables() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, abigail) = blog().await?;
    let others = User::query().except(&taylor).pluck("name").await?;
    assert_eq!(others.all(), &[json!("Abigail")]);
    assert_eq!(
        User::query()
            .except(vec![taylor.clone(), abigail.clone()])
            .count()
            .await?,
        0
    );
    assert_eq!(User::on_write_connection().count().await?, 2);
    assert_eq!(User::joining_table::<Role>(), "role_user");
    assert_eq!(Writer::joining_table::<Article>(), "article_writer");
    Ok(())
}
