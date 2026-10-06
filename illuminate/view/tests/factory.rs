mod common;

use std::sync::Arc;

use common::Views;
use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_http::{IntoResponse, Request};
use illuminate_support::{Map, MessageBag, json};
use illuminate_view::facades::{Blade, View as ViewFacade};
use illuminate_view::{Factory, View, ViewInfo, ViewServiceProvider, ViewValue, view};

#[test]
fn views_are_found_by_name() {
    let views = Views::new();
    views.add("admin.profile", "Admin {{ $name }}");
    views.add_file("legacy.blade.php", "Legacy {{ $name }}");
    views.add_file("static.html", "Static {{ $name }}");
    assert_eq!(
        views.render("admin.profile", json!({"name": "T"})),
        "Admin T"
    );
    assert_eq!(
        views.render("admin/profile", json!({"name": "T"})),
        "Admin T"
    );
    assert_eq!(views.render("legacy", json!({"name": "T"})), "Legacy T");
    // Plain HTML files are served as-is, like Laravel's file engine.
    assert_eq!(views.render("static", ()), "Static {{ $name }}");
    assert!(views.factory.exists("admin.profile"));
    assert!(!views.factory.exists("admin.missing"));
}

#[test]
fn first_existing_view() {
    let views = Views::new();
    views.add("admin", "admin view");
    let view = views.factory.first(&["custom.admin", "admin"], ()).unwrap();
    assert_eq!(view.name(), "admin");
    assert_eq!(view.render().unwrap(), "admin view");
    let error = views.factory.first(&["a", "b"], ()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "None of the views in the given array exist."
    );
}

#[test]
fn namespaced_views() {
    let views = Views::new();
    let mail = tempfile::tempdir().unwrap();
    std::fs::write(
        mail.path().join("layout.blade.html"),
        "Mail: {{ $slot ?? 'none' }}",
    )
    .unwrap();
    views.factory.add_namespace("mail", mail.path());
    assert_eq!(views.render("mail::layout", ()), "Mail: none");
    views.add("page", "@include('mail::layout')");
    assert_eq!(views.render("page", ()), "Mail: none");
}

#[test]
fn passing_data_to_views() {
    let views = Views::new();
    views.add("greeting", "{{ $name }} the {{ $occupation }} ({{ $age }})");
    let html = views
        .factory
        .make("greeting", json!({"name": "Victoria"}))
        .with("occupation", "Astronaut")
        .with_data(json!({"age": 30}))
        .render()
        .unwrap();
    assert_eq!(html, "Victoria the Astronaut (30)");

    #[derive(serde::Serialize)]
    struct Person {
        name: String,
        occupation: String,
        age: u32,
    }
    let person = Person {
        name: "Abigail".into(),
        occupation: "Engineer".into(),
        age: 28,
    };
    assert_eq!(
        views.factory.make("greeting", person).render().unwrap(),
        "Abigail the Engineer (28)"
    );
}

#[test]
fn shared_data_composers_and_creators() {
    let views = Views::new();
    views.add(
        "profile",
        "{{ $app }} {{ $count }} {{ $created }}@include('partials.footer')",
    );
    views.add("partials.footer", " [footer {{ $year }}]");
    views.add("dashboard", "{{ $app }} {{ $count ?? 'no count' }}");
    views.factory.share("app", "Laravel");
    views
        .factory
        .composer(["profile", "dashboard"], |view: &mut View| {
            view.set("count", 42);
        });
    views.factory.composer("partials.*", |view: &mut View| {
        view.set("year", 2024);
    });
    views.factory.creator("profile", |view: &mut View| {
        view.set("created", "yes");
    });
    assert_eq!(views.render("profile", ()), "Laravel 42 yes [footer 2024]");
    assert_eq!(views.render("dashboard", ()), "Laravel 42");
    assert_eq!(
        views.factory.shared("app"),
        Some(ViewValue::from("Laravel"))
    );

    let wildcard = Views::new();
    wildcard.add("a", "{{ $all }}");
    wildcard.factory.composer("*", |view: &mut View| {
        view.set("all", "every view");
    });
    assert_eq!(wildcard.render("a", ()), "every view");
}

#[test]
fn view_data_overrides_shared_data() {
    let views = Views::new();
    views.factory.share("name", "shared");
    assert_eq!(
        views.inline("{{ $name }}", json!({"name": "local"})),
        "local"
    );
    assert_eq!(views.inline("{{ $name }}", ()), "shared");
}

#[test]
fn share_resolvers_read_the_current_request() {
    let views = Views::new();
    views.factory.share_resolver(|request: &Request| {
        let mut data = Map::new();
        data.insert("path".into(), json!(request.path()));
        data.insert(
            "errors".into(),
            json!({"default": {"email": ["The email field is required."]}}),
        );
        data
    });
    let template = "{{ $path ?? 'no request' }}|{{ $errors->first('email') }}";
    assert_eq!(views.inline(template, ()), "no request|");
    let request = Request::create("/register", "POST");
    let html = illuminate_http::with_request_sync(request, || views.inline(template, ()));
    assert_eq!(html, "register|The email field is required.");
}

#[test]
fn templates_are_cached_until_they_change() {
    let views = Views::new();
    views.add("cached", "Version 1");
    assert_eq!(views.render("cached", ()), "Version 1");
    assert_eq!(views.render("cached", ()), "Version 1");
    std::thread::sleep(std::time::Duration::from_millis(20));
    views.add("cached", "Version two");
    assert_eq!(views.render("cached", ()), "Version two");
    views.factory.blade().flush_cache();
    assert_eq!(views.render("cached", ()), "Version two");
}

#[test]
fn views_with_errors() {
    let views = Views::new();
    views.add("form", "{{ $errors->first('email') }}");
    let mut bag = MessageBag::new();
    bag.add("email", "Invalid email.");
    assert_eq!(
        views
            .factory
            .make("form", ())
            .with_errors(bag)
            .render()
            .unwrap(),
        "Invalid email."
    );
}

#[test]
fn views_render_into_responses() {
    let views = Views::new();
    views.add("welcome", "<h1>Hello, {{ $name }}</h1>");
    let response = views
        .factory
        .make("welcome", json!({"name": "James"}))
        .into_response();
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.content_string(), "<h1>Hello, James</h1>");
    assert_eq!(
        response.header("content-type").unwrap(),
        "text/html; charset=UTF-8"
    );
    let info = response.extension::<ViewInfo>().unwrap();
    assert_eq!(info.name, "welcome");
    assert_eq!(info.data, json!({"name": "James"}));
    assert!(info.has("name"));

    let response = views.factory.make("missing", ()).into_response();
    assert_eq!(response.status_code(), 500);
    assert!(response.extension::<ViewInfo>().is_none());
}

#[test]
fn custom_directives() {
    let views = Views::new();
    views.factory.blade().directive("datetime", |args| {
        let date = illuminate_support::Carbon::parse(&args[0].to_string())?;
        Ok(date.format("m/d/Y H:i"))
    });
    views.factory.blade().directive("upper", |args| {
        Ok(args
            .iter()
            .map(|a| a.to_string().to_uppercase())
            .collect::<Vec<_>>()
            .join(" "))
    });
    assert_eq!(
        views.inline(
            "<p>@datetime($when)</p>\n@upper('a', $b)\n!",
            json!({"when": "2024-03-12 15:30:00", "b": "c"})
        ),
        "<p>03/12/2024 15:30</p>\nA C!"
    );
    assert_eq!(views.factory.blade().custom_directives().len(), 2);
}

#[test]
fn the_container_facades_and_helper() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("hello.blade.html"),
        "Hello, {{ $name }}! {{ $shared }}",
    )
    .unwrap();
    let app = Arc::new(Container::new());
    let _guard = Container::set_local_instance(app.clone());
    app.instance(Repository::new(json!({"view": {"paths": [dir.path()]}})));
    ViewServiceProvider.register(&app);

    ViewFacade::share("shared", "(shared)");
    assert!(ViewFacade::exists("hello"));
    assert_eq!(
        view("hello", json!({"name": "Taylor"})).render().unwrap(),
        "Hello, Taylor! (shared)"
    );
    assert_eq!(
        ViewFacade::make("hello", json!({"name": "Abigail"}))
            .render()
            .unwrap(),
        "Hello, Abigail! (shared)"
    );

    Blade::function("greet", |args| Ok(format!("Hi {}", args[0]).into()));
    assert_eq!(
        Blade::render("{{ greet($who) }}", json!({"who": "James"})).unwrap(),
        "Hi James"
    );
    Blade::if_("admin", |args| args.first().is_some_and(|v| v.truthy()));
    assert_eq!(
        Blade::render("@admin($a)yes @endadmin", json!({"a": true})).unwrap(),
        "yes "
    );
    assert!(
        app.make::<illuminate_view::BladeCompiler>()
            .has_function("greet")
    );
}

#[test]
fn the_factory_resolves_lazily_without_a_provider() {
    let app = Arc::new(Container::new());
    let _guard = Container::set_local_instance(app.clone());
    assert_eq!(Blade::render("{{ 1 + 1 }}", ()).unwrap(), "2");
    assert!(app.bound::<Factory>());
}

#[test]
fn rendering_is_thread_safe() {
    let views = Views::new();
    views.add(
        "item",
        "@foreach ($items as $item){{ $item * $factor }},@endforeach",
    );
    let factory = views.factory.clone();
    let handles: Vec<_> = (1..=8)
        .map(|factor| {
            let factory = factory.clone();
            std::thread::spawn(move || {
                factory
                    .make("item", json!({"items": [1, 2, 3], "factor": factor}))
                    .render()
                    .unwrap()
            })
        })
        .collect();
    for (index, handle) in handles.into_iter().enumerate() {
        let factor = index + 1;
        assert_eq!(
            handle.join().unwrap(),
            format!("{},{},{},", factor, factor * 2, factor * 3)
        );
    }
}

#[tokio::test]
async fn views_render_inside_async_handlers() {
    let views = Views::new();
    views.add("async", "{{ request()->path() }}");
    let factory = views.factory.clone();
    let request = Request::create("/users/1", "GET");
    let html = illuminate_http::with_request(request, async move {
        factory.make("async", ()).render().unwrap()
    })
    .await;
    assert_eq!(html, "users/1");
}
