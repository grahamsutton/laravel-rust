//! Model factories and the fake data generator.

mod eloquent_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use eloquent_support::*;
use illuminate_database::DB;
use illuminate_database::eloquent::*;
use illuminate_support::Result;

#[tokio::test]
async fn factories_make_and_create_models() -> Result<()> {
    let (_app, _guard) = app().await;

    let made = User::factory().make_one().await?;
    assert!(!made.exists());
    assert!(made.email.contains('@'));
    assert_eq!(User::count().await?, 0);

    let users = User::factory().count(3).create().await?;
    assert_eq!(users.len(), 3);
    assert!(users.iter().all(|user| user.exists() && user.active));
    assert_eq!(User::count().await?, 3);

    let emails: std::collections::HashSet<_> = users.iter().map(|u| u.email.clone()).collect();
    assert_eq!(emails.len(), 3, "unique() never repeats a value");

    let inactive = User::factory().inactive().create_one().await?;
    assert!(!inactive.active);

    let named = User::factory()
        .state(json!({"name": "Taylor"}))
        .state_fn(|attributes| json!({"email": format!("{}@laravel.com", attributes["name"].as_str().unwrap().to_lowercase())}))
        .create_one()
        .await?;
    assert_eq!(named.email, "taylor@laravel.com");

    let raw = UserFactory::new().raw().await?;
    assert_eq!(raw.len(), 1);
    assert!(raw[0].contains_key("name"));
    Ok(())
}

#[tokio::test]
async fn sequences_cycle_through_states() -> Result<()> {
    let (_app, _guard) = app().await;

    let users = User::factory()
        .count(4)
        .sequence([json!({"active": true}), json!({"active": false})])
        .create()
        .await?;
    assert_eq!(
        users.iter().map(|u| u.active).collect::<Vec<_>>(),
        [true, false, true, false]
    );

    let users = User::factory()
        .count(3)
        .sequence_fn(|index| json!({"name": format!("User {}", index + 1)}))
        .make()
        .await?;
    assert_eq!(users[2].name, "User 3");
    Ok(())
}

#[tokio::test]
async fn factory_relationships() -> Result<()> {
    let (_app, _guard) = app().await;

    let user = User::factory()
        .has(Post::factory().count(3), "posts")
        .has(
            Post::factory().count(1).state(json!({"title": "Pinned"})),
            "posts",
        )
        .create_one()
        .await?;
    assert_eq!(user.posts().count().await?, 4);
    assert_eq!(user.posts().where_("title", "Pinned").count().await?, 1);

    let posts = Post::factory()
        .count(2)
        .for_(User::factory().state(json!({"name": "Author"})), "user")
        .create()
        .await?;
    assert_eq!(
        posts[0].user_id, posts[1].user_id,
        "every model shares the parent"
    );
    assert_eq!(posts[0].user().get().await?.unwrap().name, "Author");

    let post = Post::factory()
        .for_model(&user, "user")
        .create_one()
        .await?;
    assert_eq!(post.user_id, user.id);

    Role::create(json!({"name": "admin"})).await?;
    let member = User::factory()
        .has(RoleFactoryless::roles(2), "roles")
        .create_one()
        .await?;
    assert_eq!(member.roles().count().await?, 2);
    assert_eq!(DB::table("role_user").count().await?, 2);

    let error = User::factory()
        .has(Post::factory(), "missing")
        .create_one()
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Call to undefined relationship [missing] on model [User]."
    );
    Ok(())
}

/// A factory defined inline for roles.
#[derive(Default)]
struct RoleFactory;

impl Factory for RoleFactory {
    type Model = Role;

    fn definition(&self, faker: &mut Faker) -> Value {
        json!({"name": faker.unique(|f| f.job_title())})
    }
}

struct RoleFactoryless;

impl RoleFactoryless {
    fn roles(count: usize) -> FactoryBuilder<RoleFactory> {
        RoleFactory::new().count(count)
    }
}

#[tokio::test]
async fn factory_callbacks() -> Result<()> {
    let (_app, _guard) = app().await;
    let created = Arc::new(AtomicUsize::new(0));
    let counter = created.clone();

    let users = User::factory()
        .count(2)
        .after_making(|user| {
            user.name = format!("Made {}", user.name);
            Ok(())
        })
        .after_creating(move |user: User| {
            let counter = counter.clone();
            async move {
                user.profile().create(json!({"bio": "auto"})).await?;
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .create()
        .await?;
    assert!(users[0].name.starts_with("Made "));
    assert_eq!(created.load(Ordering::SeqCst), 2);
    assert_eq!(Profile::count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn seeded_factories_are_reproducible() -> Result<()> {
    let (_app, _guard) = app().await;
    let first = User::factory().seed(7).count(2).make().await?;
    let second = User::factory().seed(7).count(2).make().await?;
    assert_eq!(first[0].name, second[0].name);
    assert_eq!(first[1].email, second[1].email);
    Ok(())
}

#[test]
fn faker_generates_realistic_data() {
    let mut faker = Faker::seeded(1);

    assert!(faker.name().contains(' '));
    assert!(faker.safe_email().contains("@example."));
    assert!(faker.free_email().contains('@'));
    assert!(faker.word().chars().all(|c| c.is_ascii_lowercase()));
    assert_eq!(faker.words(3).len(), 3);
    assert!(faker.sentence().ends_with('.'));
    assert!(faker.paragraph().len() > 20);
    assert!(faker.text(50).chars().count() <= 50);
    assert!(faker.slug().contains('-'));
    assert!(faker.url().starts_with("https://"));
    assert_eq!(faker.ipv4().split('.').count(), 4);
    assert_eq!(faker.mac_address().len(), 17);
    assert_eq!(faker.uuid().len(), 36);
    assert_eq!(faker.uuid().chars().nth(14), Some('4'));
    assert!(faker.hex_color().starts_with('#'));
    assert_eq!(faker.currency_code().len(), 3);
    assert_eq!(faker.numerify("###-##").len(), 6);
    assert!(faker.lexify("????").chars().all(|c| c.is_ascii_lowercase()));
    assert_eq!(faker.bothify("##??").len(), 4);
    assert!(!faker.street_address().is_empty());
    assert!(!faker.city().is_empty());
    assert_eq!(faker.state_abbr().len(), 2);
    assert!(faker.postcode().len() >= 5);
    assert!(!faker.phone_number().is_empty());
    assert!(!faker.company().is_empty());
    assert!(!faker.job_title().is_empty());
    assert!(faker.password().len() >= 8);

    let number = faker.number_between(5, 10);
    assert!((5..=10).contains(&number));
    assert!(faker.random_digit() <= 9);
    let float = faker.random_float(2, 1.0, 2.0);
    assert!((1.0..=2.0).contains(&float));
    assert!(faker.boolean(100));
    assert!(!faker.boolean(0));
    assert_eq!(faker.random_element(&["only"]), "only");
    assert_eq!(faker.random_elements(&[1, 2, 3], 2).len(), 2);

    let date = faker.date_time_between("2020-01-01", "2020-12-31");
    assert_eq!(date.year(), 2020);
    assert_eq!(faker.date().len(), 10);

    // Uniqueness is tracked per call site; only ten digits exist.
    let digits: Vec<_> = (0..11)
        .map(|_| faker.try_unique(|f| f.random_digit()))
        .collect();
    assert!(digits[..10].iter().all(|digit| digit.is_ok()));
    let distinct: std::collections::HashSet<_> =
        digits[..10].iter().map(|d| *d.as_ref().unwrap()).collect();
    assert_eq!(distinct.len(), 10);
    assert!(digits[10].is_err());
    faker.reset_unique();
    assert!(faker.try_unique(|f| f.random_digit()).is_ok());

    let mut a = Faker::seeded(99);
    let mut b = Faker::seeded(99);
    assert_eq!(a.name(), b.name());
    assert_eq!(a.paragraph(), b.paragraph());
}
