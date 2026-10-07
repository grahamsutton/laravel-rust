//! Dirty tracking: models with an `Original` field know what changed, and
//! only write what changed.

mod eloquent_support;

use std::sync::{Arc, Mutex};

use eloquent_support::*;
use illuminate_database::eloquent::*;
use illuminate_database::{Connection, DB};
use illuminate_support::Result;

#[derive(Debug, Clone, Default, PartialEq, Model)]
#[table("users")]
#[fillable(name, email, password, active)]
#[hidden(password)]
struct Author {
    id: i64,
    name: String,
    email: String,
    #[hashed]
    password: String,
    active: bool,
    created_at: Option<Carbon>,
    updated_at: Option<Carbon>,

    #[computed]
    posts_count: Option<i64>,

    #[relation]
    articles: Option<Vec<Article>>,

    original: Original,
}

impl Author {
    fn articles(&self) -> HasMany<Self, Article> {
        self.has_many().foreign_key("user_id")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Model)]
#[table("posts")]
#[fillable(user_id, title, body, published, votes)]
#[soft_deletes]
struct Article {
    id: i64,
    user_id: i64,
    title: String,
    body: Option<String>,
    published: bool,
    votes: i64,
    created_at: Option<Carbon>,
    updated_at: Option<Carbon>,
    deleted_at: Option<Carbon>,
    original: Original,
}

#[derive(Debug, Clone, Default, Model)]
#[table("tickets")]
#[has_uuids]
#[fillable(title)]
struct Issue {
    id: String,
    title: String,
    created_at: Option<Carbon>,
    updated_at: Option<Carbon>,
    original: Original,
}

#[derive(Debug, Clone, Default, Model)]
#[table("settings")]
#[unguarded]
#[without_timestamps]
struct Preference {
    id: i64,
    key: String,
    value: Option<String>,
    original: Original,
}

fn connection() -> Connection {
    DB::connection("sqlite")
}

/// Run the future with a fresh query log, returning the logged SQL.
async fn log<F: std::future::Future<Output = Result<T>>, T>(future: F) -> Result<(T, Vec<String>)> {
    let connection = connection();
    connection.flush_query_log();
    connection.enable_query_log();
    let result = future.await;
    connection.disable_query_log();
    let queries = connection
        .get_query_log()
        .into_iter()
        .map(|log| log.query)
        .collect();
    connection.flush_query_log();
    Ok((result?, queries))
}

async fn author(name: &str) -> Author {
    Author::create(json!({"name": name, "email": format!("{}@laravel.com", name.to_lowercase())}))
        .await
        .unwrap()
}

#[tokio::test]
async fn retrieved_models_start_clean() -> Result<()> {
    let (_app, _guard) = app().await;
    author("Taylor").await;

    let found = Author::find_or_fail(1).await?;
    assert!(found.exists());
    assert!(!found.is_dirty());
    assert!(found.is_clean());
    assert!(!found.was_changed());
    assert!(!found.was_recently_created());
    assert!(found.get_dirty().is_empty());
    assert_eq!(found.get_original()["name"], json!("Taylor"));
    assert_eq!(
        found.get_original_attribute("email"),
        json!("taylor@laravel.com")
    );
    assert_eq!(found.get_original_attribute("missing"), Value::Null);
    assert!(found.original_is_equivalent("name"));

    for author in Author::all().await? {
        assert!(author.is_clean(), "every hydrated model is clean");
    }
    let with_articles = Author::with("articles").first().await?.unwrap();
    assert!(with_articles.is_clean());
    Ok(())
}

#[tokio::test]
async fn changed_attributes_are_dirty() -> Result<()> {
    let (_app, _guard) = app().await;
    author("Taylor").await;
    let mut user = Author::find_or_fail(1).await?;

    user.name = "Otwell".into();

    assert!(user.is_dirty());
    assert!(user.is_dirty_any("name"));
    assert!(!user.is_dirty_any("email"));
    assert!(user.is_dirty_any(["email", "name"]));
    assert!(!user.is_clean());
    assert!(!user.is_clean_all("name"));
    assert!(user.is_clean_all("email"));
    assert!(!user.is_clean_all(["email", "name"]));
    assert!(!user.original_is_equivalent("name"));
    assert_eq!(Value::Object(user.get_dirty()), json!({"name": "Otwell"}));
    assert_eq!(user.get_original_attribute("name"), json!("Taylor"));

    user.name = "Taylor".into();
    assert!(
        user.is_clean(),
        "changing an attribute back makes it clean again"
    );

    user.set_attribute("active", json!(1))?;
    user.set_attribute("active", json!(0))?;
    assert!(user.is_clean(), "values compare in storage format");
    Ok(())
}

#[tokio::test]
async fn saving_writes_only_the_dirty_columns() -> Result<()> {
    let (_app, _guard) = app().await;
    Carbon::set_thread_test_now(Some(Carbon::parse("2024-01-01 10:00:00")?));
    author("Taylor").await;
    let mut user = Author::find_or_fail(1).await?;

    Carbon::set_thread_test_now(Some(Carbon::parse("2024-02-01 12:00:00")?));
    user.name = "Otwell".into();
    let (saved, queries) = log(user.save()).await?;
    assert!(saved);
    assert_eq!(
        queries,
        ["update \"users\" set \"name\" = ?, \"updated_at\" = ? where \"id\" = ?"]
    );
    assert!(user.is_clean());
    assert!(user.was_changed());
    assert!(user.was_changed_any("name"));
    assert!(user.was_changed_any(["email", "name"]));
    assert!(!user.was_changed_any("email"));
    assert_eq!(
        Value::Object(user.get_changes()),
        json!({"name": "Otwell", "updated_at": "2024-02-01 12:00:00"})
    );
    assert_eq!(
        Value::Object(user.get_previous()),
        json!({"name": "Taylor", "updated_at": "2024-01-01 10:00:00"})
    );
    assert_eq!(user.get_original_attribute("name"), json!("Otwell"));

    let row = DB::table("users").first().await?.unwrap();
    assert_eq!(row["name"], json!("Otwell"));
    assert_eq!(row["email"], json!("taylor@laravel.com"));

    // Within the same second, the fresh timestamp equals the original one,
    // so it isn't dirty (just like Laravel).
    let (_, queries) = log(user.update(json!({"email": "otwell@laravel.com"}))).await?;
    assert_eq!(
        queries,
        ["update \"users\" set \"email\" = ? where \"id\" = ?"]
    );
    assert!(user.was_changed_any("email"));
    assert!(
        !user.was_changed_any("name"),
        "changes reflect the last save only"
    );
    Carbon::set_thread_test_now(None);
    Ok(())
}

#[tokio::test]
async fn saving_a_clean_model_skips_the_update_and_its_events() -> Result<()> {
    let (_app, _guard) = app().await;
    author("Taylor").await;
    let fired = Arc::new(Mutex::new(Vec::new()));
    for (event, name) in [
        (ModelEvent::Saving, "saving"),
        (ModelEvent::Updating, "updating"),
        (ModelEvent::Updated, "updated"),
        (ModelEvent::Saved, "saved"),
    ] {
        let fired = fired.clone();
        let record = move |_: &mut Author| fired.lock().unwrap().push(name);
        match event {
            ModelEvent::Saving => Author::saving(record),
            ModelEvent::Updating => Author::updating(record),
            ModelEvent::Updated => Author::updated(record),
            _ => Author::saved(record),
        }
    }

    let mut user = Author::find_or_fail(1).await?;
    let (saved, queries) = log(user.save()).await?;
    assert!(saved, "a clean save still succeeds");
    assert!(queries.is_empty(), "no query runs: {queries:?}");
    assert_eq!(*fired.lock().unwrap(), ["saving", "saved"]);
    assert!(!user.was_changed());

    fired.lock().unwrap().clear();
    user.active = !user.active;
    user.save().await?;
    assert_eq!(
        *fired.lock().unwrap(),
        ["saving", "updating", "updated", "saved"]
    );

    // A saving listener's changes count as dirty.
    fired.lock().unwrap().clear();
    Author::flush_event_listeners();
    Author::saving(|user: &mut Author| user.name = user.name.to_uppercase());
    let (_, queries) = log(user.save()).await?;
    assert_eq!(queries.len(), 1);
    assert_eq!(Author::find_or_fail(1).await?.name, "TAYLOR");
    Ok(())
}

#[tokio::test]
async fn models_without_an_original_field_write_every_column() -> Result<()> {
    let (_app, _guard) = app().await;
    let mut user = user("Taylor").await;

    assert!(user.is_dirty(), "untracked models are always dirty");
    assert!(user.is_dirty_any("name"));
    assert!(!user.is_clean());
    assert!(!user.was_changed());
    assert!(!user.was_recently_created());
    assert!(user.get_original().is_empty());
    assert!(user.get_changes().is_empty());
    assert!(user.get_previous().is_empty());
    assert_eq!(user.get_original_attribute("name"), Value::Null);
    assert!(!user.original_is_equivalent("name"));
    assert_eq!(user.get_dirty(), user.to_attributes());
    user.sync_original().sync_changes();
    assert!(user.get_original().is_empty(), "syncing is a no-op");
    user.discard_changes()?;

    let (saved, queries) = log(user.save()).await?;
    assert!(saved);
    assert_eq!(queries.len(), 1);
    for column in [
        "name",
        "email",
        "password",
        "active",
        "country_id",
        "updated_at",
    ] {
        assert!(
            queries[0].contains(&format!("\"{column}\" = ?")),
            "{queries:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn new_models_are_dirty_until_they_are_saved() -> Result<()> {
    let (_app, _guard) = app().await;

    let mut user = Author::template();
    assert!(!user.exists());
    assert!(user.is_dirty(), "a new model's attributes are all dirty");
    assert!(user.get_dirty().contains_key("name"));
    assert!(user.get_original().is_empty());

    user.fill(json!({"name": "Taylor", "email": "taylor@laravel.com"}))?;
    assert!(user.save().await?);
    assert!(user.exists());
    assert!(user.was_recently_created());
    assert!(user.is_clean());
    assert!(!user.was_changed(), "an insert isn't a change");
    assert_eq!(user.get_original_attribute("id"), json!(1));

    let created =
        Author::create(json!({"name": "Abigail", "email": "abigail@laravel.com"})).await?;
    assert!(created.was_recently_created());
    let found = Author::find_or_fail(created.id).await?;
    assert!(!found.was_recently_created());
    assert_eq!(found, created, "tracking state never makes models unequal");

    // Like Laravel, a new model whose key you assign is inserted.
    let mut manual = Author::template();
    manual.force_fill(json!({"id": 10, "name": "James", "email": "james@laravel.com"}))?;
    assert!(!manual.exists());
    assert!(
        !manual.clone().update(json!({"name": "Jim"})).await?,
        "only existing models update"
    );
    manual.save().await?;
    assert_eq!(Author::find_or_fail(10).await?.name, "James");
    Ok(())
}

#[tokio::test]
async fn string_keys_need_no_existence_query() -> Result<()> {
    let (_app, _guard) = app().await;
    Carbon::set_thread_test_now(Some(Carbon::parse("2024-01-01 10:00:00")?));
    let issue = Issue::create(json!({"title": "Bug"})).await?;
    let mut issue = Issue::find_or_fail(issue.id.clone()).await?;

    Carbon::set_thread_test_now(Some(Carbon::parse("2024-01-02 10:00:00")?));
    issue.title = "Feature".into();
    let (_, queries) = log(issue.save()).await?;
    assert_eq!(
        queries,
        ["update \"tickets\" set \"title\" = ?, \"updated_at\" = ? where \"id\" = ?"]
    );
    assert_eq!(
        Issue::find_or_fail(issue.id.clone()).await?.title,
        "Feature"
    );
    Carbon::set_thread_test_now(None);
    Ok(())
}

#[tokio::test]
async fn changing_the_key_updates_the_original_row() -> Result<()> {
    let (_app, _guard) = app().await;
    Preference::create(json!({"key": "theme", "value": "dark"})).await?;
    let mut preference = Preference::find_or_fail(1).await?;

    preference.id = 10;
    let (_, queries) = log(preference.save()).await?;
    assert_eq!(
        queries,
        ["update \"settings\" set \"id\" = ? where \"id\" = ?"]
    );
    assert!(Preference::find(1).await?.is_none());
    assert_eq!(Preference::find_or_fail(10).await?.key, "theme");
    assert_eq!(preference.get_original_attribute("id"), json!(10));

    preference.value = None;
    let (_, queries) = log(preference.save()).await?;
    assert_eq!(
        queries,
        ["update \"settings\" set \"value\" = ? where \"id\" = ?"]
    );
    assert_eq!(Preference::find_or_fail(10).await?.value, None);
    Ok(())
}

#[tokio::test]
async fn timestamps_respect_a_manually_set_updated_at() -> Result<()> {
    let (_app, _guard) = app().await;
    author("Taylor").await;
    let mut user = Author::find_or_fail(1).await?;

    let yesterday = Carbon::parse("2020-05-05 05:05:05")?;
    user.name = "Otwell".into();
    user.updated_at = Some(yesterday);
    user.save().await?;
    assert_eq!(
        Author::find_or_fail(1).await?.updated_at,
        Some(yesterday),
        "a dirty updated_at is left alone"
    );

    Carbon::set_thread_test_now(Some(Carbon::parse("2024-03-03 03:03:03")?));
    let (touched, queries) = log(user.touch()).await?;
    assert!(touched);
    assert_eq!(
        queries,
        ["update \"users\" set \"updated_at\" = ? where \"id\" = ?"]
    );
    assert!(user.is_clean());
    Carbon::set_thread_test_now(None);
    Ok(())
}

#[tokio::test]
async fn increments_soft_deletes_and_restores_keep_originals_in_sync() -> Result<()> {
    let (_app, _guard) = app().await;
    let tick = |time: &str| Carbon::set_thread_test_now(Some(Carbon::parse(time).unwrap()));
    tick("2024-01-01 10:00:00");
    let taylor = author("Taylor").await;
    let mut article = taylor
        .articles()
        .create(json!({"title": "Hello", "published": true}))
        .await?;
    assert!(article.is_clean());

    tick("2024-01-01 11:00:00");
    article.increment("votes", 5).await?;
    assert_eq!(article.votes, 5);
    assert!(article.is_clean(), "the incremented column is synced");
    assert!(article.was_changed_any("votes"));
    assert_eq!(article.get_previous()["votes"], json!(0));

    tick("2024-01-01 12:00:00");
    let (deleted, queries) = log(article.delete()).await?;
    assert!(deleted);
    assert_eq!(
        queries,
        ["update \"posts\" set \"deleted_at\" = ?, \"updated_at\" = ? where \"id\" = ?"]
    );
    assert!(article.trashed());
    assert!(article.exists(), "soft deleted models still exist");
    assert!(article.is_clean());

    tick("2024-01-01 13:00:00");
    let (restored, queries) = log(article.restore()).await?;
    assert!(restored);
    assert_eq!(
        queries,
        ["update \"posts\" set \"updated_at\" = ?, \"deleted_at\" = ? where \"id\" = ?"]
    );
    assert!(!article.trashed());
    assert!(article.is_clean());

    article.force_delete().await?;
    assert!(
        !article.exists(),
        "permanently deleted models no longer exist"
    );
    assert!(!article.delete().await?);
    assert_eq!(Article::with_trashed().count().await?, 0);
    Carbon::set_thread_test_now(None);
    Ok(())
}

#[tokio::test]
async fn refresh_discard_and_replicate() -> Result<()> {
    let (_app, _guard) = app().await;
    author("Taylor").await;
    let mut user = Author::find_or_fail(1).await?;
    user.name = "Otwell".into();
    user.save().await?;

    user.email = "changed@laravel.com".into();
    user.discard_changes()?;
    assert_eq!(user.email, "taylor@laravel.com");
    assert!(user.is_clean());
    assert!(!user.was_changed(), "discarding forgets the last changes");

    user.name = "Unsaved".into();
    user.save().await?;
    DB::table("users")
        .update(json!({"name": "Elsewhere"}))
        .await?;
    user.email = "dirty@laravel.com".into();
    user.refresh().await?;
    assert_eq!(user.name, "Elsewhere");
    assert_eq!(user.email, "taylor@laravel.com");
    assert!(user.is_clean());
    assert!(
        user.was_changed_any("name"),
        "refreshing keeps the last changes"
    );

    let mut copy = user.replicate().await?;
    assert!(!copy.exists());
    assert!(copy.is_dirty());
    assert!(copy.get_original().is_empty());
    copy.email = "copy@laravel.com".into();
    copy.save().await?;
    assert_eq!(copy.id, 2);
    assert_eq!(Author::count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn hashed_and_listener_changes_are_written() -> Result<()> {
    let (_app, _guard) = app().await;
    hash_using(|value| format!("$2y$12$fakehashfakehash{}", value.len()));
    Carbon::set_thread_test_now(Some(Carbon::parse("2024-01-01 10:00:00")?));
    author("Taylor").await;
    let mut user = Author::find_or_fail(1).await?;
    Carbon::set_thread_test_now(Some(Carbon::parse("2024-01-01 11:00:00")?));

    user.password = "secret".into();
    let (_, queries) = log(user.save()).await?;
    assert_eq!(
        queries,
        ["update \"users\" set \"password\" = ?, \"updated_at\" = ? where \"id\" = ?"]
    );
    assert_eq!(user.password, "$2y$12$fakehashfakehash6");
    assert!(user.is_clean());

    Author::updating(|user: &mut Author| user.active = true);
    Carbon::set_thread_test_now(Some(Carbon::parse("2024-01-01 12:00:00")?));
    user.name = "Otwell".into();
    let (_, queries) = log(user.save()).await?;
    assert_eq!(
        queries,
        ["update \"users\" set \"name\" = ?, \"active\" = ?, \"updated_at\" = ? where \"id\" = ?"],
        "changes made by updating listeners are saved"
    );
    assert!(Author::find_or_fail(1).await?.active);
    Carbon::set_thread_test_now(None);
    Ok(())
}

#[tokio::test]
async fn syncing_by_hand() -> Result<()> {
    let (_app, _guard) = app().await;
    author("Taylor").await;
    let mut user = Author::find_or_fail(1).await?;

    user.name = "Otwell".into();
    user.email = "otwell@laravel.com".into();
    user.sync_original_attributes("name");
    assert!(user.is_clean_all("name"));
    assert!(user.is_dirty_any("email"));

    user.sync_changes();
    assert_eq!(
        Value::Object(user.get_changes()),
        json!({"email": "otwell@laravel.com"})
    );
    assert_eq!(
        Value::Object(user.get_previous()),
        json!({"email": "taylor@laravel.com"})
    );

    user.sync_original();
    assert!(user.is_clean());
    Ok(())
}

#[tokio::test]
async fn the_original_field_is_never_an_attribute() -> Result<()> {
    let (_app, _guard) = app().await;
    let user = author("Taylor").await;

    assert!(!Author::columns().contains(&"original"));
    assert!(!user.to_attributes().contains_key("original"));
    assert!(!user.to_array().contains_key("original"));
    assert_eq!(user.get_attribute("original"), Value::Null);
    let json: Value = serde_json::from_str(&user.to_json())?;
    assert!(json.get("original").is_none());
    assert!(json.get("password").is_none());

    let mut model = Author::template();
    model.set_attribute("original", json!({"name": "x"}))?;
    model.fill(json!({"original": "x"}))?;
    assert!(model.get_original().is_empty());
    assert!(format!("{model:?}").contains("Original { exists: false"));
    Ok(())
}
