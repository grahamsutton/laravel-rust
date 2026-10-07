//! Eloquent relationships against a live, in-memory SQLite database.

mod eloquent_support;

use eloquent_support::*;
use illuminate_database::DB;
use illuminate_database::eloquent::*;
use illuminate_support::Result;

async fn blog() -> Result<(User, User)> {
    let taylor = user("Taylor").await;
    let abigail = user("Abigail").await;
    let first = post(&taylor, "First").await;
    let second = post(&taylor, "Second").await;
    post(&abigail, "Third").await;
    first
        .comments()
        .create(json!({"body": "Great", "approved": true}))
        .await?;
    first.comments().create(json!({"body": "Spam"})).await?;
    second
        .comments()
        .create(json!({"body": "Nice", "approved": true}))
        .await?;
    Ok((taylor, abigail))
}

#[tokio::test]
async fn has_many_and_belongs_to() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, abigail) = blog().await?;

    let posts = taylor.posts().get().await?;
    assert_eq!(
        posts.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert_eq!(taylor.posts().count().await?, 2);
    assert!(abigail.posts().exists().await?);
    assert_eq!(
        taylor
            .posts()
            .where_("title", "Second")
            .first()
            .await?
            .unwrap()
            .title,
        "Second"
    );
    assert_eq!(
        taylor.posts().latest_by("id").first().await?.unwrap().title,
        "Second"
    );
    assert_eq!(
        taylor.posts().to_sql(),
        "select * from \"posts\" where \"posts\".\"user_id\" = ? and \"posts\".\"user_id\" is not null and \"posts\".\"deleted_at\" is null"
    );

    let post = Post::find(3).await?.unwrap();
    let author = post.user().get().await?.unwrap();
    assert_eq!(author.name, "Abigail");

    let first = Post::find(1).await?.unwrap();
    assert_eq!(
        first.approved_comments().count().await?,
        1,
        "relations keep their constraints"
    );
    assert_eq!(
        first.comments().pluck("body").await?.into_vec(),
        [json!("Great"), json!("Spam")]
    );
    assert_eq!(first.comments().find(2).await?.unwrap().body, "Spam");
    assert!(
        first.comments().find(3).await?.is_none(),
        "find is scoped to the parent"
    );

    // Models without a key have no related models.
    assert!(User::template().posts().get().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn creating_and_saving_related_models() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;

    let mut draft = Post::template();
    draft.title = "Draft".into();
    taylor.posts().save(&mut draft).await?;
    assert_eq!(draft.user_id, taylor.id);

    let posts = taylor
        .posts()
        .create_many([json!({"title": "One"}), json!({"title": "Two"})])
        .await?;
    assert_eq!(posts.len(), 2);
    assert_eq!(taylor.posts().count().await?, 3);

    let made = taylor.posts().make(json!({"title": "Unsaved"}))?;
    assert_eq!(made.user_id, taylor.id);
    assert!(!made.exists());

    let found = taylor
        .posts()
        .first_or_create(json!({"title": "One"}), json!({}))
        .await?;
    assert_eq!(found.id, posts[0].id);
    let updated = taylor
        .posts()
        .update_or_create(json!({"title": "Two"}), json!({"body": "Updated"}))
        .await?;
    assert_eq!(updated.body.as_deref(), Some("Updated"));

    taylor
        .profile()
        .create(json!({"bio": "Creator of Laravel"}))
        .await?;
    assert_eq!(
        taylor.profile().get().await?.unwrap().bio.as_deref(),
        Some("Creator of Laravel")
    );

    assert_eq!(taylor.posts().update(json!({"published": true})).await?, 3);
    assert_eq!(Post::where_("published", true).count().await?, 3);
    assert_eq!(taylor.posts().delete().await?, 3);
    assert_eq!(Post::only_trashed().count().await?, 3);
    Ok(())
}

#[tokio::test]
async fn associate_and_dissociate() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;
    let abigail = user("Abigail").await;
    let mut post = post(&taylor, "Hello").await;

    post.user().associate(&mut post, &abigail)?;
    post.save().await?;
    assert_eq!(Post::find(post.id).await?.unwrap().user_id, abigail.id);

    let mut user = User::find(taylor.id).await?.unwrap();
    let country = Country::create(json!({"name": "USA"})).await?;
    user.country().associate(&mut user, &country)?;
    user.save().await?;
    assert_eq!(
        User::find(taylor.id).await?.unwrap().country_id,
        Some(country.id)
    );

    user.country().dissociate(&mut user)?;
    assert_eq!(user.country_id, None);
    Ok(())
}

#[tokio::test]
async fn eager_loading() -> Result<()> {
    let (_app, _guard) = app().await;
    let (taylor, _) = blog().await?;
    taylor.profile().create(json!({"bio": "Laravel"})).await?;

    let connection = DB::connection("sqlite");
    connection.enable_query_log();
    let users = User::with(["posts.comments", "profile"])
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(
        connection.get_query_log().len(),
        4,
        "one query per relationship"
    );

    let taylor = &users[0];
    let posts = taylor.posts.as_ref().unwrap();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].comments.as_ref().unwrap().len(), 2);
    assert_eq!(posts[1].comments.as_ref().unwrap().len(), 1);
    assert_eq!(
        taylor.profile.as_ref().unwrap().bio.as_deref(),
        Some("Laravel")
    );
    assert_eq!(users[1].posts.as_ref().unwrap().len(), 1);
    assert!(users[1].profile.is_none());

    // Loaded relations are serialized.
    let array = taylor.to_array();
    assert_eq!(array["posts"][0]["title"], json!("First"));
    assert_eq!(array["posts"][0]["comments"][1]["body"], json!("Spam"));
    assert_eq!(array["profile"]["bio"], json!("Laravel"));
    assert_eq!(taylor.loaded_relations(), ["posts", "profile"]);

    // Belongs-to and constrained relationships.
    let posts = Post::with(["user", "approved_comments"]).get().await?;
    assert_eq!(posts[2].user.as_ref().unwrap().name, "Abigail");
    assert_eq!(posts[0].approved_comments.len(), 1);
    assert!(posts[2].approved_comments.is_empty());
    assert_eq!(posts[0].to_array()["user"]["name"], json!("Taylor"));
    Ok(())
}

#[tokio::test]
async fn constrained_eager_loads_columns_and_lazy_loading() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;

    let users = User::query()
        .with_constrained("posts", |query| query.where_("title", "Second"))
        .get()
        .await?;
    assert_eq!(users[0].posts.as_ref().unwrap().len(), 1);
    assert!(users[1].posts.as_ref().unwrap().is_empty());

    let users = User::with("posts:id,user_id,title").get().await?;
    let first = &users[0].posts.as_ref().unwrap()[0];
    assert_eq!(first.title, "First");
    assert!(
        first.created_at.is_none(),
        "only the selected columns are loaded"
    );

    let users = User::with(["posts", "profile"])
        .without("profile")
        .get()
        .await?;
    assert!(users[0].profile.is_none() && users[0].posts.is_some());

    let mut taylor = User::find(1).await?.unwrap();
    taylor.load("posts.comments").await?;
    assert_eq!(
        taylor.posts.as_ref().unwrap()[0]
            .comments
            .as_ref()
            .unwrap()
            .len(),
        2
    );

    let mut users = User::all().await?;
    users.load("posts").await?;
    assert_eq!(users[0].posts.as_ref().unwrap().len(), 2);
    users.load_missing(["posts", "profile"]).await?;
    assert!(users[0].profile.is_none());

    // Refreshing keeps loaded relationships, re-loaded.
    Post::create(json!({"user_id": 1, "title": "Fourth"})).await?;
    taylor.refresh().await?;
    assert_eq!(taylor.posts.as_ref().unwrap().len(), 3);

    let error = User::with("unknown").get().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Call to undefined relationship [unknown] on model [User]."
    );
    Ok(())
}

#[tokio::test]
async fn existence_queries() -> Result<()> {
    let (_app, _guard) = app().await;
    blog().await?;
    user("James").await;

    let names = |users: Collection<User>| users.into_iter().map(|u| u.name).collect::<Vec<_>>();

    assert_eq!(
        names(User::has("posts").get().await?),
        ["Taylor", "Abigail"]
    );
    assert_eq!(names(User::doesnt_have("posts").get().await?), ["James"]);
    assert_eq!(
        names(User::query().has_op("posts", ">=", 2).get().await?),
        ["Taylor"]
    );
    assert_eq!(names(User::has("posts.comments").get().await?), ["Taylor"]);
    assert_eq!(
        names(
            User::where_has("posts", |query| query.where_("title", "Third"))
                .get()
                .await?
        ),
        ["Abigail"]
    );
    assert_eq!(
        names(
            User::query()
                .where_relation("posts", "title", "First")
                .get()
                .await?
        ),
        ["Taylor"]
    );
    assert_eq!(
        names(
            User::query()
                .where_doesnt_have("posts", |q| q.where_("title", "First"))
                .get()
                .await?
        ),
        ["Abigail", "James"]
    );
    assert_eq!(
        names(
            User::where_("name", "James")
                .or_has("posts")
                .order_by("id", "asc")
                .get()
                .await?
        ),
        ["Taylor", "Abigail", "James"]
    );
    assert_eq!(
        User::has("posts").to_sql(),
        "select * from \"users\" where exists (select * from \"posts\" where \"posts\".\"user_id\" = \"users\".\"id\" and \"posts\".\"deleted_at\" is null)"
    );

    // Soft deleted related models don't count.
    Post::where_("user_id", 2).delete().await?;
    assert_eq!(names(User::has("posts").get().await?), ["Taylor"]);

    // Relationship counts and other aggregates.
    let users = User::with_count("posts")
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(
        users.iter().map(|u| u.posts_count).collect::<Vec<_>>(),
        [Some(2), Some(0), Some(0)]
    );
    let posts = Post::query()
        .with_count_constrained("comments", |query| query.where_("approved", true))
        .get()
        .await?;
    assert_eq!(posts[0].comments_count, Some(1));
    let row = User::query()
        .with_exists("posts")
        .with_max("posts", "votes")
        .where_key(1)
        .to_base()
        .first()
        .await?
        .unwrap();
    assert_eq!(row["posts_exists"], json!(1));
    assert_eq!(row["posts_max_votes"], json!(0));

    let mut taylor = User::find(1).await?.unwrap();
    taylor.load_count("posts").await?;
    assert_eq!(taylor.posts_count, Some(2));

    let error = User::has("unknown").get().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Call to undefined relationship [unknown] on model [User].")
    );
    Ok(())
}

#[tokio::test]
async fn self_referencing_relationships() -> Result<()> {
    let (_app, _guard) = app().await;
    let root = Category::create(json!({"name": "Root"})).await?;
    let child = Category::create(json!({"name": "Child", "parent_id": root.id})).await?;
    Category::create(json!({"name": "Grandchild", "parent_id": child.id})).await?;

    let categories = Category::with(["parent", "children"])
        .order_by("id", "asc")
        .get()
        .await?;
    assert!(categories[0].parent.is_none());
    assert_eq!(categories[1].parent.as_ref().unwrap().name, "Root");
    assert_eq!(categories[0].children.as_ref().unwrap()[0].name, "Child");
    assert_eq!(categories[1].to_array()["parent"]["name"], json!("Root"));

    let with_children: Vec<String> = Category::has("children")
        .get()
        .await?
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(with_children, ["Root", "Child"]);
    let sql = Category::has("children").to_sql();
    assert!(
        sql.contains("from \"categories\" as \"laravel_reserved_"),
        "{sql}"
    );

    let leaf = Category::find(3).await?.unwrap();
    assert_eq!(leaf.parent().get().await?.unwrap().name, "Child");
    Ok(())
}

#[tokio::test]
async fn belongs_to_many() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;
    let abigail = user("Abigail").await;
    for name in ["admin", "editor", "author"] {
        Role::create(json!({"name": name})).await?;
    }

    taylor.roles().attach([1, 2]).await?;
    taylor
        .roles()
        .attach_with([3], json!({"active": false}))
        .await?;
    let roles = taylor.roles().get().await?;
    assert_eq!(
        roles.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["admin", "editor", "author"]
    );
    let pivot = roles[2].pivot.as_ref().unwrap();
    assert_eq!(pivot["user_id"], json!(1));
    assert_eq!(pivot["role_id"], json!(3));
    assert_eq!(pivot["active"], json!(0));
    assert!(
        pivot["created_at"].is_string(),
        "with_timestamps fills the pivot timestamps"
    );

    assert_eq!(taylor.roles().count().await?, 3);
    assert_eq!(taylor.roles().where_pivot("active", true).count().await?, 2);
    assert_eq!(
        taylor
            .roles()
            .where_("name", "editor")
            .first()
            .await?
            .unwrap()
            .id,
        2
    );

    assert_eq!(taylor.roles().detach([2]).await?, 1);
    let changes = taylor.roles().sync([1, 2]).await?;
    assert_eq!(changes.attached, [json!(2)]);
    assert_eq!(changes.detached, [json!(3)]);
    assert!(changes.updated.is_empty());

    let changes = taylor.roles().toggle([1, 3]).await?;
    assert_eq!(changes.attached, [json!(3)]);
    assert_eq!(changes.detached, [json!(1)]);

    let changes = taylor.roles().sync_without_detaching([1]).await?;
    assert_eq!(changes.attached, [json!(1)]);
    assert_eq!(taylor.roles().count().await?, 3);

    assert_eq!(
        taylor
            .roles()
            .update_existing_pivot(1, json!({"active": false}))
            .await?,
        1
    );
    let changes = taylor
        .roles()
        .sync_with([(1, json!({"active": true})), (2, json!({}))])
        .await?;
    assert_eq!(changes.updated, [json!(1)]);
    assert_eq!(changes.detached, [json!(3)]);

    let role = taylor.roles().create(json!({"name": "owner"})).await?;
    assert_eq!(role.id, 4);
    abigail.roles().attach([role.id]).await?;

    // Eager loading, existence and the inverse relationship.
    let users = User::with("roles").order_by("id", "asc").get().await?;
    assert_eq!(users[0].roles.as_ref().unwrap().len(), 3);
    assert_eq!(users[1].roles.as_ref().unwrap()[0].name, "owner");
    assert_eq!(
        users[1].roles.as_ref().unwrap()[0].pivot.as_ref().unwrap()["user_id"],
        json!(2)
    );
    let owners = Role::find(4).await?.unwrap().users().get().await?;
    assert_eq!(owners.len(), 2);
    assert_eq!(
        User::where_has("roles", |q| q.where_("name", "admin"))
            .count()
            .await?,
        1
    );
    let roles = Role::with_count("users")
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(roles.len(), 4);
    assert_eq!(Role::has("users").count().await?, 3);

    assert_eq!(taylor.roles().detach_all().await?, 3);
    assert_eq!(DB::table("role_user").count().await?, 1);
    Ok(())
}

#[tokio::test]
async fn has_many_through() -> Result<()> {
    let (_app, _guard) = app().await;
    let usa = Country::create(json!({"name": "USA"})).await?;
    let france = Country::create(json!({"name": "France"})).await?;
    let mut taylor = user("Taylor").await;
    let mut abigail = user("Abigail").await;
    taylor.country_id = Some(usa.id);
    taylor.save().await?;
    abigail.country_id = Some(france.id);
    abigail.save().await?;
    post(&taylor, "One").await;
    post(&taylor, "Two").await;
    post(&abigail, "Trois").await;

    let posts = usa.posts().get().await?;
    assert_eq!(
        posts.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
        ["One", "Two"]
    );
    assert_eq!(usa.posts().count().await?, 2);
    assert_eq!(usa.latest_post().get().await?.unwrap().title, "Two");

    let countries = Country::with(["posts", "latest_post", "users"])
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(countries[0].posts.as_ref().unwrap().len(), 2);
    assert_eq!(countries[1].posts.as_ref().unwrap()[0].title, "Trois");
    assert_eq!(countries[1].latest_post.as_ref().unwrap().title, "Trois");
    assert_eq!(countries[0].users.as_ref().unwrap()[0].name, "Taylor");

    assert_eq!(Country::query().has_op("posts", ">", 1).count().await?, 1);
    let counts = Country::with_count("posts").to_base().get().await?;
    assert_eq!(counts[0]["posts_count"], json!(2));
    Ok(())
}

#[tokio::test]
async fn polymorphic_relationships() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;
    let post = post(&taylor, "Hello").await;

    taylor.image().create(json!({"url": "taylor.png"})).await?;
    post.image().create(json!({"url": "post.png"})).await?;

    let image = taylor.image().get().await?.unwrap();
    assert_eq!(image.imageable_type, "User");
    assert_eq!(image.imageable_id, taylor.id);
    assert_eq!(post.image().get().await?.unwrap().url, "post.png");

    let images = Image::with(["imageable_user", "imageable_post"])
        .order_by("id", "asc")
        .get()
        .await?;
    assert_eq!(images[0].imageable_user.as_ref().unwrap().name, "Taylor");
    assert!(
        images[0].imageable_post.is_none(),
        "only parents of the right type are loaded"
    );
    assert_eq!(images[1].imageable_post.as_ref().unwrap().title, "Hello");
    assert!(images[1].imageable_user.is_none());

    assert_eq!(images[1].imageable_post().get().await?.unwrap().id, post.id);
    assert!(images[1].imageable_user().get().await?.is_none());

    let users = User::with("image").get().await?;
    assert_eq!(users[0].image.as_ref().unwrap().url, "taylor.png");
    assert_eq!(User::has("image").count().await?, 1);
    assert_eq!(Image::has("imageable_post").count().await?, 1);

    morph_map([("user", "User")]);
    let avatar = taylor.image().make(json!({"url": "new.png"}))?;
    assert_eq!(avatar.imageable_type, "user");
    Ok(())
}

#[tokio::test]
async fn relation_pagination() -> Result<()> {
    let (_app, _guard) = app().await;
    let taylor = user("Taylor").await;
    for index in 1..=5 {
        post(&taylor, &format!("Post {index}")).await;
    }
    let page = taylor.posts().paginate(2).await?;
    assert_eq!(page.total(), 5);
    assert_eq!(page.items().len(), 2);
    let simple = taylor.posts().simple_paginate(10).await?;
    assert_eq!(simple.count(), 5);
    Ok(())
}
