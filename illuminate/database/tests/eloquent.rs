//! Eloquent models against a live, in-memory SQLite database: CRUD,
//! attributes, serialization, events, scopes and soft deletes.

mod eloquent_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use eloquent_support::*;
use illuminate_database::eloquent::*;
use illuminate_database::{DB, MultipleRecordsFoundException};
use illuminate_support::Result;

#[tokio::test]
async fn conventions_come_from_the_derive() {
    let (_app, _guard) = app().await;

    assert_eq!(User::table(), "users");
    assert_eq!(User::class_name(), "User");
    assert_eq!(User::primary_key(), "id");
    assert_eq!(User::key_type(), KeyType::Int);
    assert!(User::incrementing());
    assert!(User::timestamps());
    assert_eq!(User::get_foreign_key(), "user_id");
    assert_eq!(User::per_page(), 15);
    assert_eq!(
        User::relation_names(),
        &["posts", "profile", "roles", "country", "image"]
    );
    assert!(User::columns().contains(&"password"));
    assert!(!User::columns().contains(&"posts_count"));
    assert_eq!(User::hashed_attributes(), &["password"]);
    assert_eq!(Category::table(), "categories");
    assert_eq!(Ticket::key_type(), KeyType::String);
    assert!(!Ticket::incrementing());
    assert_eq!(Ticket::unique_ids(), UniqueIds::Uuid);
    assert!(!Setting::timestamps());
    assert_eq!(Flight::guarded(), &["*"]);
    assert!(Post::soft_deletes());
}

#[tokio::test]
async fn models_can_be_created_found_updated_and_deleted() -> Result<()> {
    let (_app, _guard) = app().await;

    let mut user = User::create(json!({"name": "Taylor", "email": "taylor@laravel.com"})).await?;
    assert_eq!(user.id, 1);
    assert!(user.exists());
    assert!(user.created_at.is_some());
    assert_eq!(user.created_at, user.updated_at);

    let found = User::find(1).await?.unwrap();
    assert_eq!(found.name, "Taylor");
    assert!(
        !found.active,
        "the field's default is written, not the column's"
    );
    assert!(User::find(99).await?.is_none());
    assert_eq!(
        User::find("1").await?.unwrap().id,
        1,
        "string keys are normalized"
    );

    user.name = "Taylor Otwell".into();
    assert!(user.save().await?);
    assert_eq!(User::find_or_fail(1).await?.name, "Taylor Otwell");

    assert!(user.update(json!({"email": "otwell@laravel.com"})).await?);
    assert_eq!(
        User::first_where("email", "otwell@laravel.com")
            .await?
            .unwrap()
            .id,
        1
    );

    assert!(user.delete().await?);
    assert_eq!(User::count().await?, 0);
    assert!(!Flight::template().exists());
    Ok(())
}

#[tokio::test]
async fn find_or_fail_raises_model_not_found() -> Result<()> {
    let (_app, _guard) = app().await;

    let error = User::find_or_fail(42).await.unwrap_err();
    let not_found = error.downcast_ref::<ModelNotFoundException>().unwrap();
    assert_eq!(not_found.get_model(), "User");
    assert_eq!(error.to_string(), "No query results for model [User] 42");

    let error = User::query().first_or_fail().await.unwrap_err();
    assert_eq!(error.to_string(), "No query results for model [User].");
    Ok(())
}

#[tokio::test]
async fn query_builder_methods_are_forwarded() -> Result<()> {
    let (_app, _guard) = app().await;
    for name in ["Taylor", "Abigail", "James", "Dayle"] {
        user(name).await;
    }
    User::query()
        .where_("name", "James")
        .update(json!({"active": false}))
        .await?;

    let names: Vec<String> = User::where_("active", true)
        .order_by("name", "asc")
        .get()
        .await?
        .into_iter()
        .map(|user| user.name)
        .collect();
    assert_eq!(names, ["Abigail", "Dayle", "Taylor"]);

    assert_eq!(
        User::where_in("name", ["Taylor", "James"]).count().await?,
        2
    );
    assert_eq!(User::query().scope(User::active).count().await?, 3);
    assert_eq!(
        User::where_op("id", ">", 2).pluck("name").await?.into_vec(),
        [json!("James"), json!("Dayle")]
    );
    assert_eq!(User::query().max("id").await?, json!(4));
    assert!(User::where_("name", "Taylor").exists().await?);
    assert_eq!(User::find_many([1, 3]).await?.len(), 2);
    assert_eq!(User::limit(2).get().await?.len(), 2);
    assert_eq!(
        User::where_("name", "Taylor").value("email").await?,
        Some(json!("taylor@laravel.com"))
    );

    let error = User::query().sole().await.unwrap_err();
    assert!(
        error
            .downcast_ref::<MultipleRecordsFoundException>()
            .is_some()
    );
    assert_eq!(User::where_("name", "Taylor").sole().await?.id, 1);

    let users = User::query()
        .when(true, |query| query.where_("name", "Dayle"))
        .get()
        .await?;
    assert_eq!(users.len(), 1);
    Ok(())
}

#[tokio::test]
async fn timestamps_are_maintained() -> Result<()> {
    let (_app, _guard) = app().await;

    // The test runs on a single thread, so a thread-local "now" is safe.
    Carbon::set_thread_test_now(Some(Carbon::parse("2024-01-01 10:00:00")?));
    let mut user = User::create(json!({"name": "Taylor", "email": "taylor@laravel.com"})).await?;
    assert_eq!(
        user.created_at.unwrap().to_date_time_string(),
        "2024-01-01 10:00:00"
    );

    let row = DB::table("users").first().await?.unwrap();
    assert_eq!(row["created_at"], json!("2024-01-01 10:00:00"));

    Carbon::set_thread_test_now(Some(Carbon::parse("2024-02-01 12:30:00")?));
    user.touch().await?;
    assert_eq!(
        user.updated_at.unwrap().to_date_time_string(),
        "2024-02-01 12:30:00"
    );
    assert_eq!(
        user.created_at.unwrap().to_date_time_string(),
        "2024-01-01 10:00:00"
    );

    let fresh = user.fresh().await?.unwrap();
    assert_eq!(
        fresh.updated_at.unwrap().to_date_time_string(),
        "2024-02-01 12:30:00"
    );
    assert_eq!(User::latest().first().await?.unwrap().id, 1);
    Carbon::set_thread_test_now(None);

    let setting = Setting::create(json!({"key": "theme", "value": "dark"})).await?;
    assert_eq!(setting.id, 1, "models without timestamps save fine");
    Ok(())
}

#[tokio::test]
async fn mass_assignment_is_guarded() -> Result<()> {
    let (_app, _guard) = app().await;

    let user =
        User::create(json!({"name": "Taylor", "email": "taylor@laravel.com", "id": 50})).await?;
    assert_eq!(user.id, 1, "attributes that aren't fillable are discarded");

    let error = Flight::create(json!({"name": "Oceanic 815"}))
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<MassAssignmentException>().is_some());
    assert_eq!(
        error.to_string(),
        "Add [name] to fillable property to allow mass assignment on [Flight]."
    );

    let flight = Flight::force_create(json!({"name": "Oceanic 815"})).await?;
    assert_eq!(flight.name, "Oceanic 815");

    let flight = unguarded(Flight::create(json!({"name": "Ajira 316"}))).await?;
    assert_eq!(flight.name, "Ajira 316");

    let setting = Setting::create(json!({"key": "locale", "value": "en"})).await?;
    assert_eq!(setting.key, "locale", "unguarded models accept everything");

    let mut model = User::template();
    model.fill([("name", "Abigail"), ("password", "secret")])?;
    assert_eq!(model.name, "Abigail");
    assert!(User::is_fillable("email"));
    assert!(!User::is_fillable("id"));
    Ok(())
}

#[tokio::test]
async fn attributes_are_cast_from_their_column_types() -> Result<()> {
    let (_app, _guard) = app().await;

    let user = User::create(json!({
        "name": "Taylor",
        "email": "taylor@laravel.com",
        "active": false,
        "options": ["admin", "beta"],
    }))
    .await?;

    let raw = DB::table("users").first().await?.unwrap();
    assert_eq!(raw["active"], json!(0));
    assert_eq!(raw["options"], json!("[\"admin\",\"beta\"]"));

    let found = User::find(user.id).await?.unwrap();
    assert!(!found.active);
    assert_eq!(
        found.options,
        Some(vec!["admin".to_string(), "beta".to_string()])
    );
    assert!(found.created_at.is_some());
    assert_eq!(found.get_attribute("active"), json!(false));
    assert_eq!(found.get_key(), json!(user.id));

    let mut model = User::template();
    model.set_attribute("active", json!("1"))?;
    assert!(model.active);
    let error = model
        .set_attribute("country_id", json!("not a number"))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Unable to cast attribute [country_id] on model [User]")
    );
    Ok(())
}

#[tokio::test]
async fn serialization_respects_hidden_visible_and_appends() -> Result<()> {
    let (_app, _guard) = app().await;

    let user = User::create(
        json!({"name": "Taylor", "email": "taylor@laravel.com", "password": "secret"}),
    )
    .await?;
    let array = user.to_array();
    assert!(!array.contains_key("password"));
    assert!(
        !array.contains_key("posts"),
        "unloaded relations aren't serialized"
    );
    assert_eq!(array["name"], json!("Taylor"));
    assert!(
        array["created_at"].as_str().unwrap().contains('T'),
        "dates serialize as ISO-8601"
    );

    let member = Member::find(1).await?.unwrap();
    let json: Value = serde_json::from_str(&member.to_json())?;
    assert_eq!(
        json,
        json!({"id": 1, "name": "Taylor", "display_name": "Taylor <taylor@laravel.com>"})
    );
    assert_eq!(
        member.get_attribute("display_name"),
        json!("Taylor <taylor@laravel.com>")
    );
    assert_eq!(serde_json::to_value(&member)?, json);
    Ok(())
}

#[tokio::test]
async fn hashed_attributes_use_the_hash_hook() -> Result<()> {
    let (_app, _guard) = app().await;
    hash_using(|value| format!("$2y$12$fakehashfakehash{}", value.len()));

    let mut user = User::create(
        json!({"name": "Taylor", "email": "taylor@laravel.com", "password": "secret"}),
    )
    .await?;
    assert_eq!(user.password, "$2y$12$fakehashfakehash6");

    user.name = "Taylor Otwell".into();
    user.save().await?;
    assert_eq!(
        user.password, "$2y$12$fakehashfakehash6",
        "hashed values aren't hashed again"
    );

    user.password = "new-password".into();
    user.save().await?;
    assert_eq!(
        User::find(1).await?.unwrap().password,
        "$2y$12$fakehashfakehash12"
    );
    assert!(is_hashed(&user.password));
    Ok(())
}

#[tokio::test]
async fn uuid_and_ulid_keys_are_generated() -> Result<()> {
    let (_app, _guard) = app().await;

    let ticket = Ticket::create(json!({"title": "Bug"})).await?;
    assert_eq!(ticket.id.len(), 36);
    assert_eq!(ticket.id, ticket.id.to_lowercase());
    assert_eq!(Ticket::find(ticket.id.clone()).await?.unwrap().title, "Bug");

    let mut ticket = Ticket::find_or_fail(ticket.id.clone()).await?;
    ticket.title = "Feature".into();
    ticket.save().await?;
    assert_eq!(
        Ticket::count().await?,
        1,
        "existing string keys update instead of inserting"
    );
    assert_eq!(Ticket::first().await?.unwrap().title, "Feature");

    let order = Order::create(json!({"total": 100})).await?;
    assert_eq!(order.id.len(), 26);
    assert_eq!(order.id, order.id.to_lowercase());
    assert_eq!(order.route_key_value(), order.id);
    Ok(())
}

#[tokio::test]
async fn route_model_binding() -> Result<()> {
    let (_app, _guard) = app().await;
    user("Taylor").await;

    let found = User::resolve_route_binding("1", None).await?.unwrap();
    assert_eq!(found.name, "Taylor");
    assert_eq!(found.route_key_value(), "1");
    assert_eq!(User::route_key_name(), "id");

    let member = Member::resolve_route_binding("taylor@laravel.com", None)
        .await?
        .unwrap();
    assert_eq!(member.id, 1);
    assert_eq!(Member::route_key_name(), "email");

    let by_field = User::resolve_route_binding("Taylor", Some("name"))
        .await?
        .unwrap();
    assert_eq!(by_field.id, 1);

    let error = User::resolve_route_binding_or_fail("7", None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "No query results for model [User] 7");
    Ok(())
}

#[tokio::test]
async fn first_or_create_and_update_or_create() -> Result<()> {
    let (_app, _guard) = app().await;

    let first = User::first_or_create(
        json!({"email": "taylor@laravel.com"}),
        json!({"name": "Taylor"}),
    )
    .await?;
    let again = User::first_or_create(
        json!({"email": "taylor@laravel.com"}),
        json!({"name": "Other"}),
    )
    .await?;
    assert_eq!(first.id, again.id);
    assert_eq!(again.name, "Taylor");

    let new = User::first_or_new(
        json!({"email": "abigail@laravel.com"}),
        json!({"name": "Abigail"}),
    )
    .await?;
    assert!(!new.exists());
    assert_eq!(new.name, "Abigail");

    let updated = User::update_or_create(
        json!({"email": "taylor@laravel.com"}),
        json!({"name": "Otwell"}),
    )
    .await?;
    assert_eq!(updated.id, first.id);
    assert_eq!(User::find(first.id).await?.unwrap().name, "Otwell");

    let created = User::update_or_create(
        json!({"email": "james@laravel.com"}),
        json!({"name": "James"}),
    )
    .await?;
    assert_eq!(created.id, 2);
    assert_eq!(User::count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn destroy_increment_replicate_and_refresh() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;
    user("Abigail").await;
    user("James").await;

    let mut post = post(&taylor, "Hello").await;
    post.increment("votes", 5).await?;
    assert_eq!(post.votes, 5);
    post.decrement("votes", 2).await?;
    assert_eq!(Post::find(post.id).await?.unwrap().votes, 3);

    Post::query()
        .where_key(post.id)
        .increment("votes", 10)
        .await?;
    post.refresh().await?;
    assert_eq!(post.votes, 13);

    let copy = post.replicate().await?;
    assert!(!copy.exists());
    assert_eq!(copy.title, "Hello");
    assert!(copy.created_at.is_none());
    assert!(post.is(&Post::find(post.id).await?.unwrap()));
    assert!(post.is_not(&copy));

    assert_eq!(User::destroy([2, 3]).await?, 2);
    assert_eq!(User::count().await?, 1);
    Ok(())
}

#[tokio::test]
async fn soft_deletes() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;
    let mut first = post(&taylor, "First").await;
    post(&taylor, "Second").await;

    assert!(first.delete().await?);
    assert!(first.trashed());
    assert!(first.deleted_at.is_some());
    assert_eq!(Post::count().await?, 1);
    assert_eq!(Post::with_trashed().count().await?, 2);
    assert_eq!(Post::only_trashed().count().await?, 1);
    assert!(Post::find(first.id).await?.is_none());
    assert_eq!(DB::table("posts").count().await?, 2, "the row is kept");

    assert!(first.restore().await?);
    assert!(!first.trashed());
    assert_eq!(Post::count().await?, 2);

    assert_eq!(
        Post::where_("title", "Second").delete().await?,
        1,
        "mass deletes are soft"
    );
    assert_eq!(Post::only_trashed().count().await?, 1);
    assert_eq!(Post::only_trashed().restore().await?, 1);

    first.force_delete().await?;
    assert_eq!(Post::with_trashed().count().await?, 1);

    // `or` constraints are grouped so the soft delete scope still applies.
    let mut second = Post::first().await?.unwrap();
    second.delete().await?;
    let sql = Post::where_("title", "First")
        .or_where("title", "Second")
        .to_sql();
    assert_eq!(
        sql,
        "select * from \"posts\" where (\"title\" = ? or \"title\" = ?) and \"posts\".\"deleted_at\" is null"
    );
    assert_eq!(
        Post::where_("title", "First")
            .or_where("title", "Second")
            .count()
            .await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn global_scopes() -> Result<()> {
    let (_app, _guard) = app().await;
    user("Taylor").await;
    let mut james = user("James").await;
    james.active = false;
    james.save().await?;

    User::add_global_scope("active", |query| query.where_("active", true));
    assert_eq!(User::count().await?, 1);
    assert_eq!(User::without_global_scope("active").count().await?, 2);
    assert_eq!(User::without_global_scopes().count().await?, 2);
    assert_eq!(
        User::where_("name", "Taylor")
            .or_where("name", "James")
            .to_sql(),
        "select * from \"users\" where (\"name\" = ? or \"name\" = ?) and \"active\" = ?"
    );
    assert_eq!(
        User::where_("name", "Taylor")
            .or_where("name", "James")
            .count()
            .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn closure_listeners_and_cancellation() -> Result<()> {
    let (_app, _guard) = app().await;
    let saved = Arc::new(AtomicUsize::new(0));

    User::saving(|user: &mut User| {
        user.name = user.name.trim().to_string();
    });
    User::creating(|user: &mut User| user.name != "Blocked");
    let counter = saved.clone();
    User::saved(move |_: &mut User| {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    User::deleting(|user: &mut User| -> Result<bool> { Ok(user.name != "Taylor") });

    let mut taylor =
        User::create(json!({"name": "  Taylor  ", "email": "taylor@laravel.com"})).await?;
    assert_eq!(taylor.name, "Taylor");
    assert_eq!(saved.load(Ordering::SeqCst), 1);

    let mut blocked = User::template();
    blocked.fill(json!({"name": "Blocked", "email": "blocked@laravel.com"}))?;
    assert!(
        !blocked.save().await?,
        "a creating listener returning false cancels the insert"
    );
    assert_eq!(User::count().await?, 1);
    assert_eq!(saved.load(Ordering::SeqCst), 1);

    assert!(
        !taylor.delete().await?,
        "a deleting listener returning false cancels the delete"
    );
    assert_eq!(User::count().await?, 1);

    taylor.name = "Changed".into();
    taylor.save_quietly().await?;
    assert_eq!(
        saved.load(Ordering::SeqCst),
        1,
        "save_quietly fires no events"
    );

    without_events(async {
        let mut user = User::find(1).await.unwrap().unwrap();
        user.save().await.unwrap();
    })
    .await;
    assert_eq!(saved.load(Ordering::SeqCst), 1);

    User::flush_event_listeners();
    taylor.name = "Taylor".into();
    assert!(taylor.delete().await?);
    Ok(())
}

#[derive(Default)]
struct PostObserver {
    events: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl Observer<Post> for PostObserver {
    async fn creating(&self, post: &mut Post) -> Result<bool> {
        self.events
            .lock()
            .unwrap()
            .push(format!("creating:{}", post.title));
        Ok(post.title != "Forbidden")
    }

    async fn created(&self, post: &mut Post) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("created:{}", post.id));
        // Observers may query the database.
        let count = Post::count().await?;
        self.events.lock().unwrap().push(format!("count:{count}"));
        Ok(())
    }

    async fn updating(&self, post: &mut Post) -> Result<bool> {
        post.title = post.title.to_uppercase();
        Ok(true)
    }

    async fn trashed(&self, post: &mut Post) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("trashed:{}", post.id));
        Ok(())
    }

    async fn retrieved(&self, post: &mut Post) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("retrieved:{}", post.id));
        Ok(())
    }
}

#[tokio::test]
async fn observers() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    Post::observe(PostObserver {
        events: events.clone(),
    });

    let mut post = post(&taylor, "Hello").await;
    let error = taylor.posts().create(json!({"title": "Forbidden"})).await;
    assert!(error.is_ok(), "a cancelled save isn't an error");
    assert_eq!(Post::count().await?, 1);

    post.title = "updated".into();
    post.save().await?;
    assert_eq!(post.title, "UPDATED");
    post.delete().await?;
    Post::with_trashed().get().await?;

    assert_eq!(
        *events.lock().unwrap(),
        [
            "creating:Hello",
            "created:1",
            "count:1",
            "creating:Forbidden",
            "trashed:1",
            "retrieved:1",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn chunking_and_pagination() -> Result<()> {
    let (_app, _guard) = app().await;
    for index in 1..=5 {
        user(&format!("User{index}")).await;
    }

    let mut seen = Vec::new();
    User::query()
        .chunk(2, |users, page| {
            seen.push((page, users.len()));
            async { Ok(true) }
        })
        .await?;
    assert_eq!(seen, [(1, 2), (2, 2), (3, 1)]);

    let mut ids = Vec::new();
    User::query()
        .each(2, |user| {
            ids.push(user.id);
            async { Ok(true) }
        })
        .await?;
    assert_eq!(ids, [1, 2, 3, 4, 5]);

    let mut by_id = 0;
    User::query()
        .chunk_by_id(3, |users, _| {
            by_id += users.len();
            async { Ok(true) }
        })
        .await?;
    assert_eq!(by_id, 5);

    let page = User::query().paginate_with(2, "page", Some(2)).await?;
    assert_eq!(page.total(), 5);
    assert_eq!(page.last_page(), 3);
    assert_eq!(
        page.items().iter().map(|u| u.id).collect::<Vec<_>>(),
        [3, 4]
    );
    let json = page.to_array();
    assert_eq!(json["current_page"], json!(2));
    assert_eq!(json["per_page"], json!(2));
    assert_eq!(json["data"][0]["name"], json!("User3"));
    assert!(
        json["data"][0].get("password").is_none(),
        "hidden attributes stay hidden"
    );

    let first = User::paginate(None).await?;
    assert_eq!(first.per_page(), 15);
    assert_eq!(first.count(), 5);

    let simple = User::query()
        .simple_paginate_with(2, "page", Some(3))
        .await?;
    assert_eq!(simple.count(), 1);
    assert!(!simple.has_more_pages());
    let simple = User::simple_paginate(4).await?;
    assert!(simple.has_more_pages());
    Ok(())
}

#[tokio::test]
async fn collections_of_models() -> Result<()> {
    let (_app, _guard) = app().await;
    for name in ["Taylor", "Abigail", "James"] {
        user(name).await;
    }

    let users = User::all().await?;
    assert_eq!(users.model_keys(), [json!(1), json!(2), json!(3)]);
    assert_eq!(users.find(2).unwrap().name, "Abigail");
    assert!(users.contains_key(3));
    assert!(!users.contains_key(4));
    assert_eq!(users.clone().except_keys([1]).len(), 2);
    assert_eq!(users.clone().only_keys([1, 3]).len(), 2);
    assert_eq!(users.to_query().count().await?, 3);

    User::where_("name", "James")
        .update(json!({"name": "Jim"}))
        .await?;
    let fresh = users.fresh().await?;
    assert_eq!(fresh.find(3).unwrap().name, "Jim");

    let json: Value = serde_json::from_str(&users.to_json())?;
    assert_eq!(json.as_array().unwrap().len(), 3);
    assert!(json[0].get("password").is_none());
    Ok(())
}

#[tokio::test]
async fn models_use_the_named_connection_and_transactions() -> Result<()> {
    let (_app, _guard) = app().await;

    let result: Result<()> = DB::transaction(|| async {
        user("Taylor").await;
        anyhow::bail!("rollback");
    })
    .await;
    assert!(result.is_err());
    assert_eq!(
        User::count().await?,
        0,
        "model writes participate in transactions"
    );

    user("Abigail").await;
    assert_eq!(User::on("sqlite").count().await?, 1);
    assert_eq!(User::get_connection().get_name(), "sqlite");
    assert!(
        User::query()
            .to_sql()
            .starts_with("select * from \"users\"")
    );
    Ok(())
}

#[derive(Debug, Clone, Default, Model)]
#[table("tickets")]
#[has_uuids]
#[fillable(title)]
#[observed_by(UppercaseTitles)]
struct ObservedTicket {
    id: String,
    title: String,
    created_at: Option<Carbon>,
    updated_at: Option<Carbon>,
}

struct UppercaseTitles;

#[async_trait]
impl Observer<ObservedTicket> for UppercaseTitles {
    async fn creating(&self, ticket: &mut ObservedTicket) -> Result<bool> {
        ticket.title = ticket.title.to_uppercase();
        Ok(true)
    }
}

#[tokio::test]
async fn observed_by_registers_observers_when_the_model_boots() -> Result<()> {
    let (_app, _guard) = app().await;

    let ticket = ObservedTicket::create(json!({"title": "bug"})).await?;
    assert_eq!(ticket.title, "BUG");
    assert_eq!(Ticket::find(ticket.id).await?.unwrap().title, "BUG");
    Ok(())
}

fn assert_send<T: Send>(_: T) {}

#[tokio::test]
async fn model_futures_are_send() {
    let (_app, _guard) = app().await;
    let mut user = User::template();

    assert_send(User::find(1));
    assert_send(User::all());
    assert_send(User::create(json!({})));
    assert_send(User::paginate(10));
    assert_send(User::query().get());
    assert_send(User::with("posts").first());
    assert_send(User::resolve_route_binding("1", None));
    assert_send(user.save());
    assert_send(user.delete());
    assert_send(user.load("posts"));
    assert_send(user.posts().get());
    assert_send(User::factory().create());
}
