//! Global scope objects (`Scope`, `#[scoped_by]`) and model events
//! dispatched by name through the application's event dispatcher.

mod eloquent_support;

use std::sync::{Arc, Mutex};

use eloquent_support::*;
use illuminate_database::eloquent::*;
use illuminate_support::Result;

// ----------------------------------------------------------------------
// Scopes
// ----------------------------------------------------------------------

/// Only active models (works for any model with an `active` column).
struct ActiveScope;

impl<M: Model> Scope<M> for ActiveScope {
    fn apply(&self, query: Builder<M>) -> Builder<M> {
        query.where_("active", true)
    }
}

struct PublishedScope;

impl Scope<Story> for PublishedScope {
    fn apply(&self, query: Builder<Story>) -> Builder<Story> {
        query.where_("published", true)
    }
}

/// A scope with state.
struct MinimumVotes(i64);

impl Scope<Story> for MinimumVotes {
    fn apply(&self, query: Builder<Story>) -> Builder<Story> {
        query.where_op("votes", ">=", self.0)
    }
}

#[derive(Debug, Clone, Default, Model)]
#[table("posts")]
#[fillable(user_id, title, published, votes)]
#[soft_deletes]
#[scoped_by(PublishedScope, MinimumVotes(5))]
struct Story {
    id: i64,
    user_id: i64,
    title: String,
    published: bool,
    votes: i64,
    deleted_at: Option<Carbon>,
}

async fn stories() {
    for (title, published, votes) in [
        ("Draft", false, 10),
        ("Unpopular", true, 1),
        ("Popular", true, 7),
        ("Viral", true, 100),
    ] {
        Post::create(json!({"user_id": 1, "title": title, "published": published, "votes": votes}))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn scope_objects_are_global_scopes() -> Result<()> {
    let (_app, _guard) = app().await;
    user("Taylor").await;
    let mut james = user("James").await;
    james.active = false;
    james.save().await?;

    assert!(!User::has_global_scope("ActiveScope"));
    User::add_global_scope_object(ActiveScope);
    assert!(User::has_global_scope("ActiveScope"));

    assert_eq!(User::count().await?, 1);
    assert_eq!(
        User::without_global_scope_object::<ActiveScope>()
            .count()
            .await?,
        2
    );
    assert_eq!(
        User::query()
            .without_global_scope("ActiveScope")
            .count()
            .await?,
        2,
        "scope objects are named after their type"
    );
    assert_eq!(User::without_global_scopes().count().await?, 2);
    assert_eq!(
        User::where_("name", "Taylor")
            .or_where("name", "James")
            .to_sql(),
        "select * from \"users\" where (\"name\" = ? or \"name\" = ?) and \"active\" = ?"
    );

    // Registering a scope again replaces it.
    User::add_global_scope_object(ActiveScope);
    assert_eq!(User::count().await?, 1);
    Ok(())
}

#[tokio::test]
async fn scoped_by_applies_scopes_when_the_model_boots() -> Result<()> {
    let (_app, _guard) = app().await;
    stories().await;

    let titles: Vec<String> = Story::query()
        .order_by("id", "asc")
        .get()
        .await?
        .into_iter()
        .map(|story| story.title)
        .collect();
    assert_eq!(titles, ["Popular", "Viral"]);
    assert!(Story::has_global_scope("PublishedScope"));
    assert!(Story::has_global_scope("MinimumVotes"));

    assert_eq!(
        Story::without_global_scope_object::<MinimumVotes>()
            .count()
            .await?,
        3
    );
    assert_eq!(
        Story::query()
            .without_global_scope_object::<PublishedScope>()
            .without_global_scope_object::<MinimumVotes>()
            .count()
            .await?,
        4
    );
    assert_eq!(
        Story::where_("title", "Draft")
            .or_where("title", "Viral")
            .to_sql(),
        "select * from \"posts\" where (\"title\" = ? or \"title\" = ?) and \"published\" = ? and \"votes\" >= ? and \"posts\".\"deleted_at\" is null"
    );

    Post::where_("title", "Viral").delete().await?;
    assert_eq!(
        Story::count().await?,
        1,
        "the soft delete scope still applies"
    );
    assert_eq!(Story::with_trashed().count().await?, 2);
    Ok(())
}

// ----------------------------------------------------------------------
// The event dispatcher
// ----------------------------------------------------------------------

type Dispatched = Arc<Mutex<Vec<(String, Value)>>>;

/// Records every dispatched event, halting the ones it's told to.
#[derive(Clone, Default)]
struct RecordingDispatcher {
    dispatched: Dispatched,
    listening: Option<&'static str>,
    halt: Option<&'static str>,
}

#[async_trait]
impl EventDispatcher for RecordingDispatcher {
    fn has_listeners(&self, event: &str) -> bool {
        self.listening
            .is_none_or(|prefix| event.starts_with(prefix))
    }

    async fn until(&self, event: &str, payload: Value) -> Result<bool> {
        self.dispatched
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
        Ok(self.halt != Some(event))
    }
}

impl RecordingDispatcher {
    fn names(&self) -> Vec<String> {
        self.dispatched
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn clear(&self) {
        self.dispatched.lock().unwrap().clear();
    }
}

#[tokio::test]
async fn model_events_are_dispatched_by_name() -> Result<()> {
    let (_app, _guard) = app().await;
    let events = RecordingDispatcher::default();
    assert!(!has_event_dispatcher());
    set_event_dispatcher(events.clone());
    assert!(has_event_dispatcher());

    let mut taylor = User::create(json!({"name": "Taylor", "email": "taylor@laravel.com"})).await?;
    assert_eq!(
        events.names(),
        [
            "eloquent.saving: User",
            "eloquent.creating: User",
            "eloquent.created: User",
            "eloquent.saved: User",
        ]
    );
    let (_, payload) = events.dispatched.lock().unwrap()[2].clone();
    assert_eq!(payload["id"], json!(1));
    assert_eq!(payload["name"], json!("Taylor"));

    events.clear();
    taylor.name = "Otwell".into();
    taylor.save().await?;
    User::find(1).await?;
    taylor.delete().await?;
    assert_eq!(
        events.names(),
        [
            "eloquent.saving: User",
            "eloquent.updating: User",
            "eloquent.updated: User",
            "eloquent.saved: User",
            "eloquent.retrieved: User",
            "eloquent.deleting: User",
            "eloquent.deleted: User",
        ]
    );
    assert_eq!(
        ModelEvent::Updated.name_for::<User>(),
        "eloquent.updated: User"
    );

    events.clear();
    let mut post = post(&user("Abigail").await, "Hello").await;
    events.clear();
    post.delete().await?;
    post.restore().await?;
    post.force_delete().await?;
    let names = events.names();
    assert!(names.contains(&"eloquent.trashed: Post".to_string()));
    assert!(names.contains(&"eloquent.restoring: Post".to_string()));
    assert!(names.contains(&"eloquent.restored: Post".to_string()));
    assert!(names.contains(&"eloquent.forceDeleting: Post".to_string()));
    assert!(names.contains(&"eloquent.forceDeleted: Post".to_string()));

    events.clear();
    post.replicate().await?;
    assert_eq!(events.names(), ["eloquent.replicating: Post"]);

    unset_event_dispatcher();
    assert!(!has_event_dispatcher());
    events.clear();
    user("James").await;
    assert!(events.names().is_empty());
    Ok(())
}

#[tokio::test]
async fn dispatched_listeners_can_halt_operations() -> Result<()> {
    let (_app, _guard) = app().await;
    let events = RecordingDispatcher {
        halt: Some("eloquent.creating: User"),
        ..Default::default()
    };
    set_event_dispatcher(events.clone());

    let mut user = User::template();
    user.fill(json!({"name": "Taylor", "email": "taylor@laravel.com"}))?;
    assert!(!user.save().await?, "halting creating cancels the insert");
    assert_eq!(User::count().await?, 0);
    assert_eq!(
        events.names(),
        ["eloquent.saving: User", "eloquent.creating: User"]
    );

    // Non-halting events can't cancel anything.
    let events = RecordingDispatcher {
        halt: Some("eloquent.created: User"),
        ..Default::default()
    };
    set_event_dispatcher(events.clone());
    assert!(user.save().await?);
    assert_eq!(User::count().await?, 1);
    assert!(events.names().contains(&"eloquent.saved: User".to_string()));
    Ok(())
}

#[tokio::test]
async fn observers_run_before_the_dispatcher() -> Result<()> {
    let (_app, _guard) = app().await;
    let events = RecordingDispatcher::default();
    set_event_dispatcher(events.clone());

    let log = events.dispatched.clone();
    User::creating(move |user: &mut User| {
        log.lock()
            .unwrap()
            .push(("closure".to_string(), json!(user.name)));
        user.name = user.name.to_uppercase();
    });
    User::create(json!({"name": "taylor", "email": "taylor@laravel.com"})).await?;

    let dispatched = events.dispatched.lock().unwrap().clone();
    assert_eq!(dispatched[1], ("closure".to_string(), json!("taylor")));
    assert_eq!(dispatched[2].0, "eloquent.creating: User");
    assert_eq!(
        dispatched[2].1["name"],
        json!("TAYLOR"),
        "the payload reflects the observers' changes"
    );

    // A listener cancelling the event stops it before the dispatcher.
    User::flush_event_listeners();
    User::creating(|_: &mut User| false);
    events.clear();
    User::create(json!({"name": "James", "email": "james@laravel.com"})).await?;
    assert_eq!(events.names(), ["eloquent.saving: User"]);
    Ok(())
}

#[tokio::test]
async fn events_nobody_listens_for_are_skipped() -> Result<()> {
    let (_app, _guard) = app().await;
    let events = RecordingDispatcher {
        listening: Some("eloquent.created: "),
        ..Default::default()
    };
    set_event_dispatcher(events.clone());

    user("Taylor").await;
    User::all().await?;
    assert_eq!(events.names(), ["eloquent.created: User"]);

    events.clear();
    without_events(async {
        user("Abigail").await;
    })
    .await;
    assert!(events.names().is_empty(), "muted events aren't dispatched");
    Ok(())
}

#[tokio::test]
async fn dispatchers_belong_to_the_application() -> Result<()> {
    let events = RecordingDispatcher::default();
    {
        let (_app, _guard) = app().await;
        set_event_dispatcher(events.clone());
        assert!(has_event_dispatcher());
    }
    let (_app, _guard) = app().await;
    assert!(
        !has_event_dispatcher(),
        "each container has its own dispatcher"
    );
    user("Taylor").await;
    assert!(events.names().is_empty());
    Ok(())
}
