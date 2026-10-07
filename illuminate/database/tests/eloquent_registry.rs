//! The model registry (`model:show`) and pruning (`model:prune`).

mod eloquent_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eloquent_support::*;
use illuminate_container::try_app;
use illuminate_database::DB;
use illuminate_database::eloquent::registry::{self, FieldKind};
use illuminate_database::eloquent::*;
use illuminate_support::Result;

/// What the pruning hooks saw, bound in each test's container.
#[derive(Default)]
struct PruneLog(Mutex<Vec<String>>);

impl PruneLog {
    fn titles(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Unpublished posts are pruned one by one.
#[derive(Debug, Clone, Default, Model)]
#[table("posts")]
#[fillable(user_id, title, published, votes)]
#[soft_deletes]
#[observed_by(DraftObserver)]
struct Draft {
    id: i64,
    user_id: i64,
    title: String,
    published: bool,
    votes: i64,
    created_at: Option<Carbon>,
    updated_at: Option<Carbon>,
    deleted_at: Option<Carbon>,
}

impl Prunable for Draft {
    fn prunable() -> Builder<Self> {
        Draft::where_("published", false)
    }

    async fn pruning(&mut self) -> Result<()> {
        if self.title == "Broken" {
            anyhow::bail!("Unable to prune [{}]", self.title);
        }
        if let Some(log) = try_app::<PruneLog>() {
            log.0.lock().unwrap().push(self.title.clone());
        }
        Ok(())
    }
}

struct DraftObserver;

impl Observer<Draft> for DraftObserver {}

/// Unapproved comments are pruned with mass deletes.
#[derive(Debug, Clone, Default, Model)]
#[table("comments")]
#[fillable(post_id, body, approved)]
struct SpamComment {
    id: i64,
    post_id: i64,
    body: String,
    approved: bool,
    created_at: Option<Carbon>,
    updated_at: Option<Carbon>,
}

impl MassPrunable for SpamComment {
    fn prunable() -> Builder<Self> {
        SpamComment::where_("approved", false)
    }
}

/// A model whose table doesn't exist.
#[derive(Debug, Clone, Default, Model)]
#[table("missing_things")]
#[fillable(label)]
struct Thing {
    id: u64,
    label: String,
    tags: Vec<String>,
    original: Original,
}

/// Generic models can't be registered, but still derive fine.
#[derive(Debug, Clone, Default, Model)]
#[table("flights")]
#[allow(dead_code)]
struct Generic<T: Clone + Default + Send + Sync + 'static> {
    id: i64,
    name: String,
    #[computed]
    marker: std::marker::PhantomData<T>,
}

async fn drafts(titles: &[(&str, bool)]) {
    for (title, published) in titles {
        Post::create(json!({"user_id": 1, "title": title, "published": published}))
            .await
            .unwrap();
    }
}

fn registered(name: &str) -> &'static registry::RegisteredModel {
    registry::find(name).unwrap_or_else(|| panic!("[{name}] is registered"))
}

// ----------------------------------------------------------------------
// The registry
// ----------------------------------------------------------------------

#[test]
fn every_model_is_registered() {
    let names: Vec<&str> = registry::models()
        .iter()
        .map(|model| model.class_name())
        .collect();
    for name in [
        "Draft",
        "SpamComment",
        "Thing",
        "User",
        "Post",
        "Comment",
        "Flight",
        "Member",
    ] {
        assert!(names.contains(&name), "{name} is registered: {names:?}");
    }
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "models are sorted by class name");
    assert!(
        !names.contains(&"Generic"),
        "generic models aren't registered"
    );
    let _ = Generic::<u8>::table();
}

#[test]
fn models_are_found_by_name() {
    assert_eq!(registered("Draft").class_name(), "Draft");
    assert_eq!(
        registered("draft").class_name(),
        "Draft",
        "case-insensitively"
    );
    assert_eq!(registered("App\\Models\\Draft").class_name(), "Draft");
    assert_eq!(registered("eloquent_registry::Draft").class_name(), "Draft");
    assert_eq!(registered("eloquent_support::User").class_name(), "User");
    assert!(registry::find("Nope").is_none());
    assert!(
        registry::find("other::Draft").is_some(),
        "falls back to the class name"
    );

    let draft = registered("Draft");
    assert_eq!(draft.module_path(), "eloquent_registry");
    assert_eq!(draft.qualified_name(), "eloquent_registry::Draft");
    assert_eq!(
        registered("User").module_path(),
        "eloquent_registry::eloquent_support"
    );
}

#[test]
fn registrations_describe_the_model() {
    let user = registered("User");
    assert_eq!(user.table(), "users");
    assert_eq!(user.connection(), None);
    assert_eq!(user.primary_key(), "id");
    assert_eq!(user.key_type(), KeyType::Int);
    assert!(user.incrementing());
    assert!(user.timestamps());
    assert!(!user.soft_deletes());
    assert_eq!(user.unique_ids(), UniqueIds::None);
    assert_eq!(
        user.fillable(),
        &[
            "name",
            "email",
            "password",
            "active",
            "options",
            "country_id"
        ]
    );
    assert!(user.guarded().is_empty());
    assert_eq!(user.hidden(), &["password"]);
    assert!(user.visible().is_empty());
    assert!(user.appends().is_empty());
    assert_eq!(user.per_page(), 15);
    assert_eq!(user.route_key_name(), "id");
    assert_eq!(user.morph_class(), "User");
    assert!(!user.tracks_changes());
    assert!(registered("Thing").tracks_changes());

    let columns: Vec<&str> = user.columns().iter().map(|field| field.name()).collect();
    assert_eq!(
        columns,
        [
            "id",
            "name",
            "email",
            "password",
            "active",
            "options",
            "country_id",
            "created_at",
            "updated_at"
        ]
    );
    let relations: Vec<&str> = user.relations().iter().map(|field| field.name()).collect();
    assert_eq!(relations, ["posts", "profile", "roles", "country", "image"]);

    let field = |name: &str| {
        *user
            .fields()
            .iter()
            .find(|field| field.name() == name)
            .unwrap()
    };
    assert_eq!(field("created_at").rust_type(), "Option<Carbon>");
    assert_eq!(field("options").rust_type(), "Option<Vec<String>>");
    assert_eq!(field("posts").rust_type(), "Option<Vec<Post>>");
    assert_eq!(field("posts").kind(), FieldKind::Relation);
    assert_eq!(field("posts_count").kind(), FieldKind::Computed);
    assert_eq!(field("pivot").rust_type(), "Option<Value>");
    assert!(field("password").is_hashed());
    assert!(!field("name").is_hashed());
    assert!(user.is_fillable("email"));
    assert!(!user.is_fillable("id"));

    let thing = registered("Thing");
    assert!(
        thing
            .fields()
            .iter()
            .all(|field| field.name() != "original"),
        "the Original field isn't described"
    );
    assert_eq!(registered("Post").fields()[0].rust_type(), "i64");
    assert!(registered("Post").soft_deletes());
    assert_eq!(registered("Ticket").unique_ids(), UniqueIds::Uuid);
    assert_eq!(registered("Ticket").key_type(), KeyType::String);
    assert_eq!(registered("Member").appends(), &["display_name"]);
    assert_eq!(registered("Draft").observers(), &["DraftObserver"]);
    assert!(registered("Draft").scopes().is_empty());
}

#[test]
fn prunable_models_are_detected() {
    assert_eq!(registered("Draft").prune_kind(), Some(PruneKind::Prunable));
    assert_eq!(
        registered("SpamComment").prune_kind(),
        Some(PruneKind::MassPrunable)
    );
    assert_eq!(registered("Flight").prune_kind(), None);
    assert!(!registered("Flight").is_prunable());
    assert_eq!(PruneKind::MassPrunable.name(), "MassPrunable");
    assert_eq!(
        format!("{:?}", registered("Draft").pruner().unwrap()),
        "Pruner { kind: Prunable }"
    );

    let prunable: Vec<&str> = registry::prunable()
        .iter()
        .map(|model| model.class_name())
        .collect();
    assert_eq!(prunable, ["Draft", "SpamComment"]);
}

#[test]
fn models_to_prune_follows_the_command_options() {
    let names = |models: Vec<&'static registry::RegisteredModel>| -> Vec<&'static str> {
        models.iter().map(|model| model.class_name()).collect()
    };

    let none: [&str; 0] = [];
    assert_eq!(
        names(registry::models_to_prune(&none, &none).unwrap()),
        ["Draft", "SpamComment"]
    );
    assert_eq!(
        names(registry::models_to_prune(&none, &["Draft"]).unwrap()),
        ["SpamComment"]
    );
    assert_eq!(
        names(registry::models_to_prune(&["Flight", "Nope", "draft", "Draft"], &none).unwrap()),
        ["Flight", "Draft"],
        "named models are pruned even when they aren't prunable (they prune nothing)"
    );
    let models = vec!["Draft".to_string()];
    let except = vec!["SpamComment".to_string()];
    let error = registry::models_to_prune(&models, &except).unwrap_err();
    assert_eq!(
        error.to_string(),
        "The --model and --except options cannot be combined."
    );
}

#[tokio::test]
async fn models_are_inspected_against_their_table() -> Result<()> {
    let (_app, _guard) = app().await;
    User::created(|_: &mut User| {});
    User::observe(UserAudit);

    let info = registered("User").inspect(None).await?;
    assert_eq!(info.class, "User");
    assert_eq!(info.database, "sqlite");
    assert_eq!(info.table, "users");
    assert_eq!(info.policy, None);
    assert_eq!(info.prunable, None);

    let attribute = |name: &str| {
        info.attributes
            .iter()
            .find(|a| a.name == name)
            .unwrap()
            .clone()
    };
    let id = attribute("id");
    assert!(id.increments);
    assert_eq!(id.type_.as_deref(), Some("integer"));
    assert!(!id.fillable);
    assert_eq!(id.cast.as_deref(), Some("i64"));
    let email = attribute("email");
    assert_eq!(email.unique, Some(true));
    assert_eq!(email.nullable, Some(false));
    assert!(email.fillable);
    assert_eq!(email.type_.as_deref(), Some("varchar"));
    assert_eq!(attribute("name").unique, Some(false));
    assert!(attribute("password").hidden);
    assert_eq!(attribute("password").default.as_deref(), Some("''"));
    assert_eq!(attribute("country_id").nullable, Some(true));
    assert_eq!(
        attribute("created_at").cast.as_deref(),
        Some("Option<Carbon>")
    );
    assert_eq!(info.attributes.len(), 9);

    let relations: Vec<(String, String, String)> = info
        .relations
        .iter()
        .map(|r| (r.name.clone(), r.type_.clone(), r.related.clone()))
        .collect();
    assert_eq!(
        relations,
        [
            ("posts".into(), "HasMany".into(), "Post".into()),
            ("profile".into(), "HasOne".into(), "Profile".into()),
            ("roles".into(), "BelongsToMany".into(), "Role".into()),
            ("country".into(), "BelongsTo".into(), "Country".into()),
            ("image".into(), "HasOne".into(), "Image".into()),
        ]
    );

    let observers: Vec<(String, Vec<String>)> = info
        .observers
        .iter()
        .map(|o| (o.event.clone(), o.observer.clone()))
        .collect();
    assert_eq!(
        observers,
        [
            ("created".to_string(), vec!["Closure".to_string()]),
            ("*".to_string(), vec!["UserAudit".to_string()]),
        ]
    );

    let json = serde_json::to_value(&info)?;
    assert_eq!(json["attributes"][0]["type"], json!("integer"));
    assert_eq!(json["relations"][0]["type"], json!("HasMany"));
    assert_eq!(json["class"], json!("User"));
    Ok(())
}

struct UserAudit;

impl Observer<User> for UserAudit {}

#[tokio::test]
async fn inspection_covers_accessors_scopes_and_pruning() -> Result<()> {
    let (_app, _guard) = app().await;

    let member = registered("Member").inspect(Some("sqlite")).await?;
    let display = member
        .attributes
        .iter()
        .find(|attribute| attribute.name == "display_name")
        .unwrap();
    assert_eq!(display.appended, Some(true));
    assert_eq!(display.cast.as_deref(), Some("accessor"));
    assert_eq!(display.type_, None);
    let email = member
        .attributes
        .iter()
        .find(|a| a.name == "email")
        .unwrap();
    assert!(email.hidden, "attributes missing from `visible` are hidden");
    let password = member
        .attributes
        .iter()
        .find(|a| a.name == "password")
        .unwrap();
    assert_eq!(password.cast, None, "columns without a field have no cast");

    Draft::add_global_scope("titled", |query| query.where_not_null("title"));
    let draft = registered("Draft").inspect(None).await?;
    assert_eq!(draft.prunable, Some(PruneKind::Prunable));
    assert_eq!(draft.scopes, ["titled"]);
    assert_eq!(
        draft.observers[0].observer,
        ["DraftObserver"],
        "inspecting boots the model"
    );

    let thing = registered("Thing").inspect(None).await?;
    assert_eq!(thing.table, "missing_things");
    let names: Vec<&str> = thing.attributes.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        ["id", "label", "tags"],
        "missing tables describe the fields"
    );
    assert_eq!(thing.attributes[2].cast.as_deref(), Some("Vec<String>"));
    assert!(thing.attributes[0].increments);
    assert_eq!(thing.attributes[0].type_, None);
    Ok(())
}

// ----------------------------------------------------------------------
// Pruning
// ----------------------------------------------------------------------

#[tokio::test]
async fn prunable_models_are_deleted_one_at_a_time() -> Result<()> {
    let (app, _guard) = app().await;
    app.instance(PruneLog::default());
    drafts(&[
        ("One", false),
        ("Published", true),
        ("Two", false),
        ("Three", false),
        ("Live", true),
        ("Four", false),
        ("Five", false),
    ])
    .await;
    Post::find_or_fail(3).await?.delete().await?;

    let deleting = Arc::new(AtomicUsize::new(0));
    let counter = deleting.clone();
    Draft::force_deleted(move |_: &mut Draft| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let draft = registered("Draft");
    assert_eq!(
        draft.pretend_to_prune().await?,
        5,
        "soft deleted models count"
    );
    assert_eq!(
        DB::table("posts").count().await?,
        7,
        "pretending deletes nothing"
    );

    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = progress.clone();
    let pruned = draft
        .prune_with_progress(2, move |total| seen.lock().unwrap().push(total))
        .await?;
    assert_eq!(pruned, 5);
    assert_eq!(*progress.lock().unwrap(), [2, 4, 5]);
    assert_eq!(
        deleting.load(Ordering::SeqCst),
        5,
        "every model fires its events"
    );
    assert_eq!(
        app.make::<PruneLog>().titles(),
        ["One", "Two", "Three", "Four", "Five"],
        "pruning runs before each delete"
    );
    assert_eq!(
        DB::table("posts").count().await?,
        2,
        "soft deleted models are force deleted"
    );
    assert_eq!(Post::count().await?, 2);

    assert_eq!(draft.prune(2).await?, 0, "nothing left to prune");
    assert_eq!(draft.pretend_to_prune().await?, 0);
    Ok(())
}

#[tokio::test]
async fn cancelled_deletes_are_not_counted() -> Result<()> {
    let (_app, _guard) = app().await;
    drafts(&[("One", false), ("Keep", false), ("Two", false)]).await;
    Draft::deleting(|draft: &mut Draft| draft.title != "Keep");

    assert_eq!(Draft::prune_all(1000).await?, 2);
    let titles = Post::pluck("title").await?.into_vec();
    assert_eq!(titles, [json!("Keep")]);
    Ok(())
}

#[tokio::test]
async fn pruning_errors_are_reported_or_returned() -> Result<()> {
    let (app, _guard) = app().await;
    app.instance(PruneLog::default());
    drafts(&[("One", false), ("Broken", false), ("Two", false)]).await;

    let error = Draft::prune_all(1000).await.unwrap_err();
    assert_eq!(error.to_string(), "Unable to prune [Broken]");
    assert_eq!(Post::count().await?, 2, "pruning stops without a reporter");

    let reported = Arc::new(Mutex::new(Vec::new()));
    let log = reported.clone();
    report_exceptions_using(move |error| log.lock().unwrap().push(error.to_string()));
    assert_eq!(Draft::prune_all(1000).await?, 1);
    assert_eq!(*reported.lock().unwrap(), ["Unable to prune [Broken]"]);
    assert_eq!(
        Post::pluck("title").await?.into_vec(),
        [json!("Broken")],
        "the others are pruned"
    );
    Ok(())
}

#[tokio::test]
async fn a_single_model_can_be_pruned() -> Result<()> {
    let (_app, _guard) = app().await;
    drafts(&[("One", false)]).await;

    let mut draft = Draft::find_or_fail(1).await?;
    assert!(draft.prune().await?);
    assert_eq!(DB::table("posts").count().await?, 0);
    Ok(())
}

#[tokio::test]
async fn mass_prunable_models_are_deleted_with_queries() -> Result<()> {
    let (_app, _guard) = app().await;
    for (index, approved) in [false, true, false, false, true, false, false]
        .iter()
        .enumerate()
    {
        Comment::create(
            json!({"post_id": 1, "body": format!("Comment {index}"), "approved": approved}),
        )
        .await?;
    }
    let deleting = Arc::new(AtomicUsize::new(0));
    let counter = deleting.clone();
    SpamComment::deleting(move |_: &mut SpamComment| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let spam = registered("SpamComment");
    assert_eq!(spam.pretend_to_prune().await?, 5);

    let connection = DB::connection("sqlite");
    connection.enable_query_log();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = progress.clone();
    let pruned = spam
        .prune_with_progress(2, move |total| seen.lock().unwrap().push(total))
        .await?;
    connection.disable_query_log();

    assert_eq!(pruned, 5);
    assert_eq!(*progress.lock().unwrap(), [2, 4, 5]);
    assert_eq!(deleting.load(Ordering::SeqCst), 0, "no model events fire");
    assert_eq!(Comment::count().await?, 2);
    let queries = connection.get_query_log();
    assert_eq!(
        queries.len(),
        4,
        "one delete per chunk, until nothing is left"
    );
    assert!(
        queries
            .iter()
            .all(|log| log.query.starts_with("delete from \"comments\""))
    );

    assert_eq!(SpamComment::prune_all(1000).await?, 0);
    assert_eq!(registered("Flight").prune(1000).await?, 0);
    assert_eq!(registered("Flight").pretend_to_prune().await?, 0);
    Ok(())
}

#[tokio::test]
async fn the_prune_command_flow() -> Result<()> {
    let (_app, _guard) = app().await;
    drafts(&[("Draft", false), ("Live", true)]).await;
    Comment::create(json!({"post_id": 1, "body": "Spam", "approved": false})).await?;

    // php artisan model:prune --pretend
    let mut report = Vec::new();
    let none: [&str; 0] = [];
    for model in registry::models_to_prune(&none, &none)? {
        let count = model.pretend_to_prune().await?;
        report.push(format!(
            "{count} [{}] records will be pruned.",
            model.class_name()
        ));
    }
    assert_eq!(
        report,
        [
            "1 [Draft] records will be pruned.",
            "1 [SpamComment] records will be pruned."
        ]
    );

    // php artisan model:prune --except=SpamComment --chunk=1000
    for model in registry::models_to_prune(&none, &["SpamComment"])? {
        assert_eq!(model.prune(1000).await?, 1);
    }
    assert_eq!(Post::count().await?, 1);
    assert_eq!(Comment::count().await?, 1);
    Ok(())
}
