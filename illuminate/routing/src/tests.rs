//! Router integration tests: registering, matching, dispatching, URL
//! generation and redirects, all through the facades.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_http::{
    HeaderMap, HeaderValue, HttpException, Json, Middleware, Next, Request, Response, async_trait,
    middleware_fn,
};
use illuminate_support::{Carbon, Result, Value, json};
use serde::Deserialize;

use crate::*;

fn app() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    RoutingServiceProvider.register(&container);
    (container, guard)
}

fn configure(container: &Container, config: Value) {
    container.instance(Repository::new(config));
}

async fn send(uri: &str, method: &str) -> Response {
    Route::router().dispatch(Request::create(uri, method)).await
}

async fn get(uri: &str) -> Response {
    send(uri, "GET").await
}

fn request_with_headers(uri: &str, method: &str, headers: &[(&'static str, &str)]) -> Request {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.insert(*name, HeaderValue::from_str(value).unwrap());
    }
    Request::create_with(uri, method, json!({}), map)
}

// ----------------------------------------------------------------------
// Matching
// ----------------------------------------------------------------------

#[tokio::test]
async fn basic_routes_are_dispatched() {
    let _app = app();
    Route::get("/", || async { "Welcome" });
    Route::get("/greeting", || async { "Hello World" });
    Route::post("/users", || async { "Created" });
    Route::match_(&["put", "patch"], "/users/{id}", || async { "Updated" });
    Route::any("/anything", || async { "Any" });

    assert_eq!(get("/").await.content_string(), "Welcome");
    assert_eq!(get("/greeting").await.content_string(), "Hello World");
    assert_eq!(get("/greeting/").await.content_string(), "Hello World");
    assert_eq!(send("/users", "POST").await.content_string(), "Created");
    assert_eq!(send("/users/1", "PATCH").await.content_string(), "Updated");
    assert_eq!(send("/users/1", "PUT").await.content_string(), "Updated");
    assert_eq!(send("/anything", "DELETE").await.content_string(), "Any");
    assert_eq!(send("/anything", "QUERY").await.content_string(), "Any");
}

#[tokio::test]
async fn head_requests_match_get_routes() {
    let _app = app();
    Route::get("/ping", || async { "pong" });
    let response = send("/ping", "HEAD").await;
    assert_eq!(response.status_code(), 200);
}

#[tokio::test]
async fn unknown_routes_are_404() {
    let _app = app();
    Route::get("/", || async { "Welcome" });
    let response = get("/missing/page").await;
    assert_eq!(response.status_code(), 404);
    let exception = response.exception().unwrap();
    assert_eq!(
        exception.downcast_ref::<HttpException>().unwrap().message(),
        "The route missing/page could not be found."
    );
}

#[tokio::test]
async fn other_verbs_produce_405_with_an_allow_header() {
    let _app = app();
    Route::get("/users", || async { "Users" });
    Route::post("/users", || async { "Created" });
    let response = send("/users", "DELETE").await;
    assert_eq!(response.status_code(), 405);
    assert_eq!(response.header("allow").unwrap(), "GET, HEAD, POST");
    assert_eq!(
        response.exception().unwrap().to_string(),
        "The DELETE method is not supported for route users. Supported methods: GET, HEAD, POST."
    );
}

#[tokio::test]
async fn options_requests_are_answered_automatically() {
    let _app = app();
    Route::get("/users", || async { "Users" });
    Route::delete("/users", || async { "Deleted" });
    let response = send("/users", "OPTIONS").await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.header("allow").unwrap(), "GET,HEAD,DELETE");

    Route::options("/explicit", || async { "Custom" });
    assert_eq!(
        send("/explicit", "OPTIONS").await.content_string(),
        "Custom"
    );
}

#[tokio::test]
async fn routes_match_in_registration_order() {
    let _app = app();
    Route::get("/photos/{photo}", || async { "show" });
    Route::get("/photos/popular", || async { "popular" });
    assert_eq!(get("/photos/popular").await.content_string(), "show");
}

#[tokio::test]
async fn form_method_spoofing_is_respected() {
    let _app = app();
    Route::put("/posts/1", || async { "Updated" });
    let request = Request::create_with(
        "/posts/1",
        "POST",
        json!({"_method": "PUT"}),
        HeaderMap::new(),
    );
    let response = Route::router().dispatch(request).await;
    assert_eq!(response.content_string(), "Updated");
}

// ----------------------------------------------------------------------
// Parameters & constraints
// ----------------------------------------------------------------------

#[tokio::test]
async fn parameters_are_bound_and_extracted() {
    let _app = app();
    Route::get(
        "/posts/{post}/comments/{comment}",
        |Path((post, comment)): Path<(u32, String)>| async move { format!("{post}:{comment}") },
    );
    assert_eq!(
        get("/posts/1/comments/great").await.content_string(),
        "1:great"
    );

    #[derive(Deserialize)]
    struct Params {
        user: String,
        id: u64,
    }
    Route::get(
        "/users/{user}/items/{id}",
        |Path(params): Path<Params>| async move { format!("{} owns {}", params.user, params.id) },
    );
    assert_eq!(
        get("/users/taylor/items/9").await.content_string(),
        "taylor owns 9"
    );
}

#[tokio::test]
async fn request_route_parameters_are_set() {
    let _app = app();
    Route::get("/users/{id}", |request: Request| async move {
        format!(
            "{} {}",
            request.route_or("id", "?"),
            request.route_name().unwrap_or_default()
        )
    })
    .name("users.show");
    assert_eq!(get("/users/5").await.content_string(), "5 users.show");
}

#[tokio::test]
async fn optional_parameters_may_be_omitted() {
    let _app = app();
    Route::get(
        "/user/{name?}",
        |Path(name): Path<Option<String>>| async move { name.unwrap_or_else(|| "John".to_string()) },
    );
    assert_eq!(get("/user").await.content_string(), "John");
    assert_eq!(get("/user/taylor").await.content_string(), "taylor");

    Route::get("/page/{number?}", |Path(number): Path<u32>| async move {
        number.to_string()
    })
    .defaults("number", "1");
    assert_eq!(get("/page").await.content_string(), "1");
    assert_eq!(get("/page/4").await.content_string(), "4");
}

#[tokio::test]
async fn constraints_restrict_matching() {
    let _app = app();
    Route::get("/user/{id}", || async { "numeric" }).where_number("id");
    Route::get("/user/{name}", || async { "alpha" }).where_alpha("name");
    Route::get("/category/{category}", || async { "category" })
        .where_in("category", &["movie", "song", "painting"]);
    Route::get("/token/{id}", || async { "uuid" }).where_uuid("id");
    Route::get("/ulid/{id}", || async { "ulid" }).where_ulid("id");
    Route::get("/code/{code}", || async { "code" }).where_alpha_numeric("code");

    assert_eq!(get("/user/42").await.content_string(), "numeric");
    assert_eq!(get("/user/taylor").await.content_string(), "alpha");
    assert_eq!(get("/user/taylor42").await.status_code(), 404);
    assert_eq!(get("/category/song").await.content_string(), "category");
    assert_eq!(get("/category/book").await.status_code(), 404);
    assert_eq!(
        get("/token/2b3d9a5e-3d7c-4b6e-8f53-1c8a9b9f0e11")
            .await
            .content_string(),
        "uuid"
    );
    assert_eq!(get("/token/nope").await.status_code(), 404);
    assert_eq!(
        get("/ulid/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .await
            .content_string(),
        "ulid"
    );
    assert_eq!(get("/code/abc123").await.content_string(), "code");
    assert_eq!(get("/code/abc-123").await.status_code(), 404);
}

#[tokio::test]
async fn where_allows_slashes_in_parameters() {
    let _app = app();
    Route::get(
        "/search/{search}",
        |Path(search): Path<String>| async move { search },
    )
    .where_("search", ".*");
    assert_eq!(get("/search/a/b/c").await.content_string(), "a/b/c");
}

#[tokio::test]
async fn global_patterns_apply_to_every_route() {
    let _app = app();
    Route::pattern("id", "[0-9]+");
    Route::get("/user/{id}", || async { "user" });
    assert_eq!(get("/user/1").await.status_code(), 200);
    assert_eq!(get("/user/abc").await.status_code(), 404);
}

#[tokio::test]
async fn invalid_parameter_types_are_404() {
    let _app = app();
    Route::get(
        "/user/{id}",
        |Path(id): Path<u64>| async move { id.to_string() },
    );
    assert_eq!(get("/user/abc").await.status_code(), 404);
}

#[tokio::test]
async fn path_shape_mismatches_are_server_errors() {
    let _app = app();
    Route::get(
        "/a/{x}/{y}",
        |Path(x): Path<u64>| async move { x.to_string() },
    );
    assert_eq!(get("/a/1/2").await.status_code(), 500);
}

#[tokio::test]
async fn binding_fields_are_recorded() {
    let _app = app();
    Route::get("/posts/{post:slug}", |route: CurrentRoute| async move {
        format!(
            "{} by {}",
            route.parameter("post").unwrap(),
            route.binding_field_for("post").unwrap()
        )
    })
    .name("posts.show");
    assert_eq!(
        get("/posts/hello-world").await.content_string(),
        "hello-world by slug"
    );
    assert_eq!(
        Route::get_by_name("posts.show").unwrap().uri(),
        "posts/{post}"
    );
}

// ----------------------------------------------------------------------
// Groups
// ----------------------------------------------------------------------

#[tokio::test]
async fn groups_share_prefixes_names_and_constraints() {
    let _app = app();
    Route::prefix("admin")
        .name("admin.")
        .where_number("id")
        .group(|| {
            Route::get("/users/{id}", || async { "admin user" }).name("users.show");
            Route::prefix("reports").name("reports.").group(|| {
                Route::get("/", || async { "reports" }).name("index");
            });
        });

    let route = Route::get_by_name("admin.users.show").unwrap();
    assert_eq!(route.uri(), "admin/users/{id}");
    assert_eq!(route.get_prefix().as_deref(), Some("admin"));
    assert_eq!(
        Route::get_by_name("admin.reports.index").unwrap().uri(),
        "admin/reports"
    );
    assert_eq!(get("/admin/users/1").await.content_string(), "admin user");
    assert_eq!(get("/admin/users/x").await.status_code(), 404);
    assert_eq!(get("/admin/reports").await.content_string(), "reports");
}

#[tokio::test]
async fn registrars_can_register_single_routes() {
    let _app = app();
    Route::middleware("auth")
        .get("/dashboard", || async { "Dashboard" })
        .name("dashboard");
    let route = Route::get_by_name("dashboard").unwrap();
    assert_eq!(route.middleware_names(), vec!["auth"]);
    assert!(!Route::router().has_group_stack());
}

#[tokio::test]
async fn subdomain_routing_captures_parameters() {
    let _app = app();
    Route::domain("{account}.example.com").group(|| {
        Route::get(
            "/user/{id}",
            |Path((account, id)): Path<(String, u32)>| async move { format!("{account}:{id}") },
        )
        .name("account.user");
    });

    let request = request_with_headers("/user/7", "GET", &[("host", "acme.example.com")]);
    let response = Route::router().dispatch(request).await;
    assert_eq!(response.content_string(), "acme:7");

    let request = request_with_headers("/user/7", "GET", &[("host", "example.org")]);
    assert_eq!(Route::router().dispatch(request).await.status_code(), 404);

    assert_eq!(
        route("account.user", json!({"account": "laravel", "id": 1})).unwrap(),
        "http://laravel.example.com/user/1"
    );
}

#[tokio::test]
async fn route_names_append_to_group_name_prefixes() {
    let _app = app();
    Route::name("admin.").group(|| {
        Route::get("/users", || async { "Users" }).name("users");
    });
    assert!(Route::has("admin.users"));
    assert!(Route::has_all(&["admin.users"]));
    assert!(!Route::has("users"));
}

// ----------------------------------------------------------------------
// Middleware
// ----------------------------------------------------------------------

fn tagging(tag: &'static str) -> Arc<dyn Middleware> {
    middleware_fn(move |request: Request, next: Next| async move {
        let mut trail = request
            .attribute("trail")
            .as_str()
            .unwrap_or_default()
            .to_string();
        trail.push_str(tag);
        request.set_attribute("trail", trail);
        let response = next.run(request).await;
        Ok(response.with_header(&format!("x-{tag}"), "1"))
    })
}

async fn trail(request: Request) -> String {
    request
        .attribute("trail")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn middleware_aliases_and_groups_run_in_order() {
    let _app = app();
    let router = Route::router();
    router.alias_middleware_instance("first", tagging("a"));
    router.alias_middleware_instance("second", tagging("b"));
    router.alias_middleware_instance("third", tagging("c"));
    router.middleware_group("web", ["first", "second"]);
    router.middleware_group("everything", ["web", "third"]);

    Route::get("/trail", trail).middleware("everything");
    let response = get("/trail").await;
    assert_eq!(response.content_string(), "abc");
    assert_eq!(response.header("x-a").as_deref(), Some("1"));

    Route::middleware("web").group(|| {
        Route::get("/without", trail).without_middleware("first");
    });
    assert_eq!(get("/without").await.content_string(), "b");
}

#[tokio::test]
async fn middleware_receives_parameters() {
    let _app = app();
    Route::alias_middleware("role", |parameters: &[String]| {
        let roles = parameters.join("|");
        middleware_fn(move |request: Request, next: Next| {
            let roles = roles.clone();
            async move {
                request.set_attribute("roles", roles);
                Ok(next.run(request).await)
            }
        })
    });
    Route::get("/posts", |request: Request| async move {
        request.attribute("roles").to_string()
    })
    .middleware("role:editor,publisher");
    assert_eq!(get("/posts").await.content_string(), "\"editor|publisher\"");
}

#[tokio::test]
async fn can_is_sugar_for_the_can_middleware() {
    let _app = app();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    Route::alias_middleware("can", move |parameters: &[String]| {
        sink.lock().unwrap().push(parameters.to_vec());
        middleware_fn(|request: Request, next: Next| async move { Ok(next.run(request).await) })
    });
    Route::put("/posts/{post}", || async { "ok" }).can("update", "post");
    assert_eq!(
        Route::get_routes()[0].middleware_names(),
        vec!["can:update,post"]
    );
    send("/posts/1", "PUT").await;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![vec!["update".to_string(), "post".to_string()]]
    );
}

#[tokio::test]
async fn unknown_middleware_is_a_clear_error() {
    let _app = app();
    Route::get("/secret", || async { "secret" }).middleware("nope");
    let response = get("/secret").await;
    assert_eq!(response.status_code(), 500);
    assert!(
        response
            .exception()
            .unwrap()
            .to_string()
            .contains("Middleware [nope] is not defined")
    );
}

#[tokio::test]
async fn recursive_groups_are_reported() {
    let _app = app();
    Route::middleware_group("loop", "loop");
    Route::get("/loop", || async { "never" }).middleware("loop");
    let response = get("/loop").await;
    assert_eq!(response.status_code(), 500);
    assert_eq!(
        response.exception().unwrap().to_string(),
        "[loop] middleware group is referencing itself."
    );
}

#[tokio::test]
async fn groups_can_be_modified() {
    let _app = app();
    let router = Route::router();
    router.middleware_group("web", ["session", "csrf"]);
    router.push_middleware_to_group("web", "bindings");
    router.push_middleware_to_group("web", "session");
    router.prepend_middleware_to_group("web", "cookies");
    router.remove_middleware_from_group("web", "csrf");
    router.push_middleware_to_group("api", "throttle:api");
    assert_eq!(
        router.get_middleware_groups()["web"],
        vec!["cookies", "session", "bindings"]
    );
    assert_eq!(router.get_middleware_groups()["api"], vec!["throttle:api"]);
    assert!(router.has_middleware_group("api"));
    router.flush_middleware_groups();
    assert!(!router.has_middleware_group("web"));
}

#[tokio::test]
async fn middleware_is_sorted_by_priority_and_deduplicated() {
    let _app = app();
    let router = Route::router();
    router.alias_middleware_instance("session", tagging("s"));
    router.alias_middleware_instance("auth", tagging("u"));
    router.alias_middleware_instance("log", tagging("l"));
    router.set_middleware_priority(["session", "auth"]);
    Route::get("/sorted", trail).middleware(["log", "auth", "session", "log"]);
    assert_eq!(get("/sorted").await.content_string(), "lsu");
    assert_eq!(
        router
            .gather_route_middleware_names(&Route::get_routes()[0])
            .unwrap(),
        vec!["log", "session", "auth"]
    );
}

struct Counter(Arc<AtomicUsize>);

#[async_trait]
impl Middleware for Counter {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(next.run(request).await)
    }

    async fn terminate(&self, _request: &Request, _response: &Response) {
        self.0.fetch_add(100, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn instances_and_closures_are_middleware_too() {
    let _app = app();
    let count = Arc::new(AtomicUsize::new(0));
    Route::get("/counted", || async { "ok" })
        .middleware(RouteMiddleware::of(Counter(count.clone())))
        .middleware(middleware_fn(|request: Request, next: Next| async move {
            let response = next.run(request).await;
            Ok(response.with_header("x-closure", "yes"))
        }));

    let request = Request::create("/counted", "GET");
    let response = Route::router().dispatch(request.clone()).await;
    assert_eq!(response.header("x-closure").as_deref(), Some("yes"));
    assert_eq!(count.load(Ordering::SeqCst), 1);

    Route::router().terminate(&request, &response).await;
    assert_eq!(count.load(Ordering::SeqCst), 101);

    let listing = Route::routes();
    assert_eq!(listing[0].middleware.len(), 2);
    assert!(listing[0].middleware[0].ends_with("Counter"));
    assert_eq!(listing[0].middleware[1], "Closure");
}

#[tokio::test]
async fn middleware_errors_are_rendered() {
    let _app = app();
    Route::get("/forbidden", || async { "never" }).middleware(middleware_fn(
        |_request: Request, _next: Next| async move { Err(HttpException::new(403).into()) },
    ));
    assert_eq!(get("/forbidden").await.status_code(), 403);
}

#[tokio::test]
async fn excluding_middleware_by_base_name() {
    let _app = app();
    let router = Route::router();
    router.alias_middleware_instance("throttle", tagging("t"));
    Route::middleware("throttle:60,1").group(|| {
        Route::get("/unthrottled", trail).without_middleware("throttle");
        Route::get("/throttled", trail);
    });
    assert_eq!(get("/unthrottled").await.content_string(), "");
    assert_eq!(get("/throttled").await.content_string(), "t");

    Route::without_middleware("throttle").group(|| {
        Route::get("/free", trail).middleware("throttle:1,1");
    });
    assert_eq!(get("/free").await.content_string(), "");
}

// ----------------------------------------------------------------------
// Handlers & extractors
// ----------------------------------------------------------------------

#[derive(Deserialize)]
struct Search {
    q: String,
    page: Option<u32>,
}

#[tokio::test]
async fn extractors_compose_in_handlers() {
    let _app = app();
    Route::get(
        "/search/{scope}",
        |Path(scope): Path<String>, Query(search): Query<Search>, request: Request| async move {
            format!(
                "{scope}:{}:{}:{}",
                search.q,
                search.page.unwrap_or(1),
                request.method()
            )
        },
    );
    assert_eq!(
        get("/search/posts?q=rust&page=2").await.content_string(),
        "posts:rust:2:GET"
    );
    assert_eq!(get("/search/posts").await.status_code(), 400);
}

#[tokio::test]
async fn json_and_input_extractors() {
    let _app = app();
    #[derive(Deserialize, serde::Serialize)]
    struct NewUser {
        name: String,
        age: u8,
    }
    Route::post(
        "/users",
        |Json(user): Json<NewUser>| async move { Json(user) },
    );
    Route::post("/form", |Input(user): Input<NewUser>| async move {
        format!("{} ({})", user.name, user.age)
    });

    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    let request = Request::create_with(
        "/users",
        "POST",
        json!({"name": "Taylor", "age": 36}),
        headers,
    );
    let response = Route::router().dispatch(request).await;
    assert_eq!(response.json_body(), json!({"name": "Taylor", "age": 36}));

    let request = Request::create_with(
        "/form",
        "POST",
        json!({"name": "Abigail", "age": "30"}),
        HeaderMap::new(),
    );
    assert_eq!(
        Route::router().dispatch(request).await.content_string(),
        "Abigail (30)"
    );

    let request = Request::create_with(
        "/form",
        "POST",
        json!({"name": "Abigail"}),
        HeaderMap::new(),
    );
    assert_eq!(Route::router().dispatch(request).await.status_code(), 422);
}

#[tokio::test]
async fn services_are_injected() {
    let (container, _guard) = app();
    struct Greeting {
        text: &'static str,
    }
    container.instance(Greeting { text: "Howdy" });
    Route::get("/hello", |greeting: Inject<Greeting>| async move {
        greeting.text
    });
    assert_eq!(get("/hello").await.content_string(), "Howdy");
}

#[tokio::test]
async fn handlers_returning_errors_are_rendered() {
    let _app = app();
    Route::get("/teapot", || async {
        Err::<&str, _>(HttpException::new(418))
    });
    Route::get("/json", || async { json!({"framework": "Laravel"}) });
    assert_eq!(get("/teapot").await.status_code(), 418);
    let response = get("/json").await;
    assert_eq!(response.header("content-type").unwrap(), "application/json");
}

struct UserController;

impl UserController {
    async fn show(Path(id): Path<u64>) -> String {
        format!("User {id}")
    }
}

#[tokio::test]
async fn controller_methods_are_handlers() {
    let _app = app();
    Route::get("/users/{id}", UserController::show).name("users.show");
    assert_eq!(get("/users/3").await.content_string(), "User 3");
    let listing = Route::routes();
    assert_eq!(listing[0].action, "UserController@show");
    assert_eq!(listing[0].methods, vec!["GET", "HEAD"]);
    assert_eq!(listing[0].uri, "users/{id}");
    assert_eq!(listing[0].name.as_deref(), Some("users.show"));
}

#[tokio::test]
async fn missing_handlers_run_when_bindings_fail() {
    let _app = app();
    struct Model;

    #[async_trait]
    impl FromRequest for Model {
        async fn from_request(request: &Request) -> Result<Self> {
            match route_parameter_for(request, "location") {
                Some((value, _)) if value == "home" => Ok(Model),
                _ => Err(HttpException::new(404).into()),
            }
        }
    }

    Route::get("/locations/{location:slug}", |_model: Model| async {
        "found"
    })
    .missing(|| async { Redirect::to("/locations") });
    assert_eq!(get("/locations/home").await.content_string(), "found");
    let response = get("/locations/mars").await;
    assert!(response.is_redirect());
    assert_eq!(response.target_url().unwrap(), "http://localhost/locations");
}

#[tokio::test]
async fn missing_model_detectors_extend_missing_handling() {
    let _app = app();
    #[derive(Debug, thiserror::Error)]
    #[error("No query results for model [Flight].")]
    struct ModelNotFound;

    struct Flight;

    #[async_trait]
    impl FromRequest for Flight {
        async fn from_request(_request: &Request) -> Result<Self> {
            Err(ModelNotFound.into())
        }
    }

    Route::get("/flights/{flight}", |_flight: Flight| async { "flight" })
        .missing(|| async { "no flight" });
    assert_eq!(get("/flights/1").await.status_code(), 500);

    Route::set_missing_model_detector(|error| error.downcast_ref::<ModelNotFound>().is_some());
    assert_eq!(get("/flights/1").await.content_string(), "no flight");
    assert!(Route::router().is_missing_model_error(&ModelNotFound.into()));
}

#[tokio::test]
async fn the_current_route_is_available() {
    let _app = app();
    Route::get("/admin/users", || async {
        format!(
            "{:?} {} {}",
            Route::current_route_name(),
            Route::is("admin.*"),
            Route::current_route_action().unwrap()
        )
    })
    .name("admin.users");
    assert_eq!(
        get("/admin/users").await.content_string(),
        "Some(\"admin.users\") true Closure"
    );
    assert!(Route::current().is_none());
}

#[tokio::test]
async fn matched_callbacks_run() {
    let _app = app();
    let matched = Arc::new(Mutex::new(Vec::new()));
    let sink = matched.clone();
    Route::matched(move |route, _request| sink.lock().unwrap().push(route.uri()));
    Route::get("/x/{id}", || async { "x" });
    get("/x/1").await;
    assert_eq!(*matched.lock().unwrap(), vec!["x/{id}".to_string()]);
}

// ----------------------------------------------------------------------
// Redirect, view & fallback routes
// ----------------------------------------------------------------------

#[tokio::test]
async fn redirect_routes() {
    let _app = app();
    Route::redirect("/here", "/there");
    Route::permanent_redirect("/old/{id}", "/new/{id}");
    Route::redirect_with_status("/temp", "https://laravel.com", 307);

    let response = get("/here").await;
    assert_eq!(response.status_code(), 302);
    assert_eq!(response.target_url().unwrap(), "/there");

    let response = send("/old/5", "POST").await;
    assert_eq!(response.status_code(), 301);
    assert_eq!(response.target_url().unwrap(), "/new/5");

    assert_eq!(
        get("/temp").await.target_url().unwrap(),
        "https://laravel.com"
    );
    assert_eq!(Route::routes()[0].action, "RedirectController");
}

#[tokio::test]
async fn view_routes_use_the_registered_renderer() {
    let _app = app();
    Route::view(
        "/welcome/{name}",
        "welcome",
        json!({"framework": "Laravel"}),
    );
    assert_eq!(get("/welcome/taylor").await.status_code(), 500);

    Route::set_view_renderer(Arc::new(|view: &str, data: Value| {
        Ok(Response::new(format!(
            "{view}: {} {}",
            data["framework"], data["name"]
        )))
    }));
    assert!(Route::router().has_view_renderer());
    assert_eq!(
        get("/welcome/taylor").await.content_string(),
        "welcome: \"Laravel\" \"taylor\""
    );
    assert_eq!(send("/welcome/taylor", "POST").await.status_code(), 405);
}

#[tokio::test]
async fn fallback_routes_catch_everything_else() {
    let _app = app();
    Route::fallback(|| async { (404, "Custom not found") });
    Route::get("/home", || async { "Home" });
    assert_eq!(get("/home").await.content_string(), "Home");
    let response = get("/nope/nothing/here").await;
    assert_eq!(response.status_code(), 404);
    assert_eq!(response.content_string(), "Custom not found");
    // Just like Laravel, the GET fallback makes other verbs a 405.
    assert_eq!(send("/nope", "POST").await.status_code(), 405);
}

// ----------------------------------------------------------------------
// Resources
// ----------------------------------------------------------------------

struct PhotoController;

#[async_trait]
impl ResourceController for PhotoController {
    async fn index(&self, _request: Request) -> Result<Response> {
        Ok(Response::new("index"))
    }

    async fn show(&self, request: Request) -> Result<Response> {
        let photo = request.route_or("photo", "");
        if photo == "404" {
            return Err(HttpException::new(404).into());
        }
        Ok(Response::new(format!("show {photo}")))
    }

    async fn update(&self, request: Request) -> Result<Response> {
        Ok(Response::new(format!(
            "update {}",
            request.route_or("photo", "")
        )))
    }
}

fn names() -> Vec<String> {
    Route::get_routes()
        .iter()
        .filter_map(|route| route.get_name())
        .collect()
}

fn listing() -> Vec<(String, String, String)> {
    Route::routes()
        .into_iter()
        .map(|route| {
            (
                route.methods.join("|"),
                route.uri,
                route.name.unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn resources_register_the_crud_routes() {
    let _app = app();
    Route::resource("photos", PhotoController);
    assert_eq!(
        listing(),
        vec![
            ("GET|HEAD".into(), "photos".into(), "photos.index".into()),
            (
                "GET|HEAD".into(),
                "photos/create".into(),
                "photos.create".into()
            ),
            ("POST".into(), "photos".into(), "photos.store".into()),
            (
                "GET|HEAD".into(),
                "photos/{photo}".into(),
                "photos.show".into()
            ),
            (
                "GET|HEAD".into(),
                "photos/{photo}/edit".into(),
                "photos.edit".into()
            ),
            (
                "PUT|PATCH".into(),
                "photos/{photo}".into(),
                "photos.update".into()
            ),
            (
                "DELETE".into(),
                "photos/{photo}".into(),
                "photos.destroy".into()
            ),
        ]
    );
    assert_eq!(get("/photos").await.content_string(), "index");
    assert_eq!(get("/photos/7").await.content_string(), "show 7");
    assert_eq!(
        send("/photos/7", "PATCH").await.content_string(),
        "update 7"
    );
    assert_eq!(get("/photos/create").await.status_code(), 404);
    assert_eq!(Route::routes()[0].action, "PhotoController@index");
}

#[tokio::test]
async fn partial_and_api_resources() {
    let _app = app();
    Route::resource("photos", PhotoController).only(&["index", "show"]);
    Route::resource("videos", PhotoController).except(&["create", "store", "update", "destroy"]);
    Route::api_resource("posts", PhotoController);
    Route::api_resource("tags", PhotoController).except(&["destroy"]);
    assert_eq!(
        names(),
        vec![
            "photos.index",
            "photos.show",
            "videos.index",
            "videos.show",
            "videos.edit",
            "posts.index",
            "posts.store",
            "posts.show",
            "posts.update",
            "posts.destroy",
            "tags.index",
            "tags.store",
            "tags.show",
            "tags.update",
        ]
    );
}

#[tokio::test]
async fn nested_and_shallow_resources() {
    let _app = app();
    Route::resource("photos.comments", PhotoController).only(&["index", "show"]);
    Route::resource("posts.comments", PhotoController)
        .shallow()
        .only(&["index", "show"]);
    assert_eq!(
        listing(),
        vec![
            (
                "GET|HEAD".into(),
                "photos/{photo}/comments".into(),
                "photos.comments.index".into()
            ),
            (
                "GET|HEAD".into(),
                "photos/{photo}/comments/{comment}".into(),
                "photos.comments.show".into()
            ),
            (
                "GET|HEAD".into(),
                "posts/{post}/comments".into(),
                "posts.comments.index".into()
            ),
            (
                "GET|HEAD".into(),
                "comments/{comment}".into(),
                "comments.show".into()
            ),
        ]
    );
}

#[tokio::test]
async fn resource_names_parameters_and_middleware() {
    let _app = app();
    Route::resource("users", PhotoController)
        .only(&["create", "show"])
        .names([("create", "users.build")])
        .parameters([("users", "admin_user")])
        .middleware("auth")
        .middleware_for(&["show"], "verified")
        .where_number("admin_user");
    let create = Route::get_by_name("users.build").unwrap();
    assert_eq!(create.middleware_names(), vec!["auth"]);
    let show = Route::get_by_name("users.show").unwrap();
    assert_eq!(show.uri(), "users/{admin_user}");
    assert_eq!(show.middleware_names(), vec!["auth", "verified"]);
    assert_eq!(show.wheres()["admin_user"], "[0-9]+");

    Route::resource("admin/posts", PhotoController).only(&["index"]);
    assert_eq!(
        Route::get_by_name("posts.index").unwrap().uri(),
        "admin/posts"
    );

    Route::resource("photos.comments", PhotoController)
        .only(&["show"])
        .scoped([("comment", "slug")]);
    let scoped = Route::get_by_name("photos.comments.show").unwrap();
    assert_eq!(scoped.binding_field_for("comment").as_deref(), Some("slug"));
    assert_eq!(scoped.binding_field_for("photo"), None);
}

#[tokio::test]
async fn resources_inside_groups() {
    let _app = app();
    Route::prefix("admin")
        .name("admin.")
        .middleware("auth")
        .group(|| {
            Route::resource("photos", PhotoController).only(&["index"]);
        });
    let route = Route::get_by_name("admin.photos.index").unwrap();
    assert_eq!(route.uri(), "admin/photos");
    assert_eq!(route.middleware_names(), vec!["auth"]);

    Route::middleware("verified")
        .resource("videos", PhotoController)
        .only(&["index"]);
    assert_eq!(
        Route::get_by_name("videos.index")
            .unwrap()
            .middleware_names(),
        vec!["verified"]
    );
}

#[tokio::test]
async fn resource_missing_handlers_cover_member_routes() {
    let _app = app();
    Route::resource("photos", PhotoController)
        .only(&["index", "show"])
        .missing(|| async { Redirect::route("photos.index", ()) });
    let response = get("/photos/404").await;
    assert!(response.is_redirect());
    assert_eq!(response.target_url().unwrap(), "http://localhost/photos");
    assert!(
        Route::get_by_name("photos.index")
            .unwrap()
            .missing_handler()
            .is_none()
    );
}

#[tokio::test]
async fn singleton_resources() {
    let _app = app();
    Route::singleton("profile", PhotoController);
    Route::singleton("photos.thumbnail", PhotoController).creatable();
    Route::api_singleton("settings", PhotoController);
    Route::singleton("avatar", PhotoController).destroyable();
    assert_eq!(
        listing(),
        vec![
            ("GET|HEAD".into(), "profile".into(), "profile.show".into()),
            (
                "GET|HEAD".into(),
                "profile/edit".into(),
                "profile.edit".into()
            ),
            (
                "PUT|PATCH".into(),
                "profile".into(),
                "profile.update".into()
            ),
            (
                "GET|HEAD".into(),
                "photos/{photo}/thumbnail".into(),
                "photos.thumbnail.show".into()
            ),
            (
                "GET|HEAD".into(),
                "photos/{photo}/thumbnail/edit".into(),
                "photos.thumbnail.edit".into()
            ),
            (
                "PUT|PATCH".into(),
                "photos/{photo}/thumbnail".into(),
                "photos.thumbnail.update".into()
            ),
            (
                "GET|HEAD".into(),
                "photos/{photo}/thumbnail/create".into(),
                "photos.thumbnail.create".into()
            ),
            (
                "POST".into(),
                "photos/{photo}/thumbnail".into(),
                "photos.thumbnail.store".into()
            ),
            (
                "DELETE".into(),
                "photos/{photo}/thumbnail".into(),
                "photos.thumbnail.destroy".into()
            ),
            ("GET|HEAD".into(), "settings".into(), "settings.show".into()),
            (
                "PUT|PATCH".into(),
                "settings".into(),
                "settings.update".into()
            ),
            ("GET|HEAD".into(), "avatar".into(), "avatar.show".into()),
            (
                "GET|HEAD".into(),
                "avatar/edit".into(),
                "avatar.edit".into()
            ),
            ("PUT|PATCH".into(), "avatar".into(), "avatar.update".into()),
            ("DELETE".into(), "avatar".into(), "avatar.destroy".into()),
        ]
    );
}

#[tokio::test]
async fn resource_verbs_can_be_localized() {
    let _app = app();
    Route::resource_verbs("crear", "editar");
    Route::resource("publicacion", PhotoController).only(&["create", "edit"]);
    let uris: Vec<String> = Route::get_routes()
        .iter()
        .map(RouteDefinition::uri)
        .collect();
    assert_eq!(
        uris,
        vec!["publicacion/crear", "publicacion/{publicacion}/editar"]
    );
}

#[tokio::test]
async fn many_resources_at_once() {
    let _app = app();
    Route::resources(vec![
        (
            "photos",
            Arc::new(PhotoController) as Arc<dyn ResourceController>,
        ),
        ("posts", Arc::new(PhotoController)),
    ]);
    Route::api_resources(vec![(
        "tags",
        Arc::new(PhotoController) as Arc<dyn ResourceController>,
    )]);
    assert_eq!(Route::get_routes().len(), 7 + 7 + 5);
}

// ----------------------------------------------------------------------
// URL generation
// ----------------------------------------------------------------------

struct Post {
    id: u64,
    slug: &'static str,
}

impl UrlRoutable for Post {
    fn route_key(&self) -> String {
        self.id.to_string()
    }

    fn route_field(&self, field: &str) -> Option<String> {
        (field == "slug").then(|| self.slug.to_string())
    }
}

#[tokio::test]
async fn urls_use_the_configured_app_url_outside_requests() {
    let (container, _guard) = app();
    assert_eq!(url("/posts/1"), "http://localhost/posts/1");
    configure(&container, json!({"app": {"url": "https://example.com/"}}));
    assert_eq!(url("posts/1"), "https://example.com/posts/1");
    assert_eq!(url("/"), "https://example.com");
    assert_eq!(secure_url("/login"), "https://example.com/login");
    assert_eq!(asset("css/app.css"), "https://example.com/css/app.css");
    assert_eq!(URL::current(), "https://example.com");
}

#[tokio::test]
async fn the_request_root_wins_during_http() {
    let (container, _guard) = app();
    configure(&container, json!({"app": {"url": "https://example.com"}}));
    Route::get("/where", |request: Request| async move {
        format!(
            "{} {} {}",
            url("/home"),
            URL::current(),
            URL::full().replace(&request.root(), "")
        )
    });
    let request = request_with_headers("/where?x=1", "GET", &[("host", "laravel.test:8000")]);
    let response = Route::router().dispatch(request).await;
    assert_eq!(
        response.content_string(),
        "http://laravel.test:8000/home http://laravel.test:8000/where /where?x=1"
    );
}

#[tokio::test]
async fn assets_use_the_asset_url() {
    let (container, _guard) = app();
    configure(
        &container,
        json!({"app": {"url": "http://example.com", "asset_url": "https://cdn.example.com"}}),
    );
    assert_eq!(
        asset("/img/logo.png"),
        "https://cdn.example.com/img/logo.png"
    );
    assert_eq!(
        URL::asset_from("http://static.test", "app.js"),
        "http://static.test/app.js"
    );
    assert_eq!(secure_asset("x.css"), "https://cdn.example.com/x.css");
}

#[tokio::test]
async fn named_routes_generate_urls() {
    let _app = app();
    Route::get("/user/{id}/profile", || async { "" }).name("profile");
    Route::get("/post/{post}/comment/{comment}", || async { "" }).name("comment.show");
    Route::get("/posts", || async { "" }).name("posts.index");

    assert_eq!(
        route("profile", json!({"id": 1})).unwrap(),
        "http://localhost/user/1/profile"
    );
    assert_eq!(
        route("profile", json!({"id": 1, "photos": "yes"})).unwrap(),
        "http://localhost/user/1/profile?photos=yes"
    );
    assert_eq!(
        route("profile", 5).unwrap(),
        "http://localhost/user/5/profile"
    );
    assert_eq!(
        route("profile", "taylor").unwrap(),
        "http://localhost/user/taylor/profile"
    );
    assert_eq!(
        route("comment.show", (1, 3)).unwrap(),
        "http://localhost/post/1/comment/3"
    );
    assert_eq!(
        route("comment.show", [("comment", 3), ("post", 1), ("page", 2)]).unwrap(),
        "http://localhost/post/1/comment/3?page=2"
    );
    assert_eq!(route("posts.index", ()).unwrap(), "http://localhost/posts");
    assert_eq!(
        route("posts.index", json!({"filter": {"tags": ["a b", "c"]}})).unwrap(),
        "http://localhost/posts?filter%5Btags%5D%5B0%5D=a%20b&filter%5Btags%5D%5B1%5D=c"
    );
    assert_eq!(
        URL::route_with("profile", 1, false).unwrap(),
        "/user/1/profile"
    );
}

#[tokio::test]
async fn missing_and_unknown_routes_are_errors() {
    let _app = app();
    Route::get("/user/{id}/profile", || async { "" }).name("profile");
    let error = route("profile", ()).unwrap_err();
    assert!(error.downcast_ref::<UrlGenerationException>().is_some());
    assert_eq!(
        error.to_string(),
        "Missing required parameter for [Route: profile] [URI: user/{id}/profile] [Missing parameter: id]."
    );
    let error = route("nope", ()).unwrap_err();
    assert!(error.downcast_ref::<RouteNotFoundException>().is_some());
    assert_eq!(error.to_string(), "Route [nope] not defined.");
}

#[tokio::test]
async fn optional_parameters_and_defaults() {
    let _app = app();
    Route::get("/archive/{year?}/{month?}", || async { "" }).name("archive");
    Route::get("/{locale}/posts", || async { "" }).name("posts.index");
    assert_eq!(route("archive", ()).unwrap(), "http://localhost/archive");
    assert_eq!(
        route("archive", 2024).unwrap(),
        "http://localhost/archive/2024"
    );
    assert_eq!(
        route("archive", (2024, 5)).unwrap(),
        "http://localhost/archive/2024/5"
    );

    URL::defaults([("locale", "en")]);
    assert_eq!(
        route("posts.index", ()).unwrap(),
        "http://localhost/en/posts"
    );
    assert_eq!(
        route("posts.index", json!({"locale": "fr"})).unwrap(),
        "http://localhost/fr/posts"
    );
}

#[tokio::test]
async fn url_defaults_are_request_scoped_during_http() {
    let _app = app();
    Route::get("/{locale}/posts", || async {
        route("posts.index", ()).unwrap()
    })
    .name("posts.index")
    .middleware(middleware_fn(|request: Request, next: Next| async move {
        URL::defaults([("locale", request.route_or("locale", "en"))]);
        Ok(next.run(request).await)
    }));
    assert_eq!(
        get("/de/posts").await.content_string(),
        "http://localhost/de/posts"
    );
    assert!(URL::generator().get_default_parameters().is_empty());
}

#[tokio::test]
async fn models_are_route_parameters() {
    let _app = app();
    Route::get("/posts/{post}", || async { "" }).name("posts.show");
    Route::get("/blog/{post:slug}", || async { "" }).name("blog.show");
    let post = Post {
        id: 42,
        slug: "hello-world",
    };
    assert_eq!(
        route("posts.show", &post).unwrap(),
        "http://localhost/posts/42"
    );
    assert_eq!(
        route("blog.show", &post).unwrap(),
        "http://localhost/blog/hello-world"
    );
    assert_eq!(
        route(
            "blog.show",
            RouteParameters::new()
                .with("post", &post)
                .with("ref", "home")
        )
        .unwrap(),
        "http://localhost/blog/hello-world?ref=home"
    );
}

#[tokio::test]
async fn parameters_are_encoded() {
    let _app = app();
    Route::get("/search/{term}", || async { "" }).name("search");
    assert_eq!(
        route("search", "rock & roll?").unwrap(),
        "http://localhost/search/rock%20&%20roll%3F"
    );
    assert_eq!(
        route("search", "café").unwrap(),
        "http://localhost/search/caf%C3%A9"
    );
}

#[tokio::test]
async fn query_urls_and_valid_urls() {
    let _app = app();
    assert_eq!(
        URL::query("/posts?sort=latest", json!({"sort": "oldest"})),
        "http://localhost/posts?sort=oldest"
    );
    assert_eq!(
        URL::to("mailto:taylor@laravel.com"),
        "mailto:taylor@laravel.com"
    );
    assert!(URL::is_valid_url("https://laravel.com"));
    assert!(URL::is_valid_url("//cdn.test/app.js"));
    assert!(!URL::is_valid_url("posts/1"));
    URL::force_https(true);
    assert_eq!(URL::to("/secure"), "https://localhost/secure");
    URL::force_root_url(Some("https://app.test/"));
    assert_eq!(URL::to("/x"), "https://app.test/x");
}

// ----------------------------------------------------------------------
// Signed URLs
// ----------------------------------------------------------------------

fn signing_app() -> (Arc<Container>, LocalInstanceGuard) {
    let (container, guard) = app();
    configure(
        &container,
        json!({"app": {"key": "base64:c2VjcmV0LWtleS1zZWNyZXQta2V5LXNlY3JldC1rZXk=", "previous_keys": ["old-key"]}}),
    );
    (container, guard)
}

#[tokio::test]
async fn signed_urls_validate() {
    let _app = signing_app();
    Route::get("/unsubscribe/{user}", |request: Request| async move {
        if URL::has_valid_signature(&request) {
            "valid"
        } else {
            "invalid"
        }
    })
    .name("unsubscribe");

    let url = URL::signed_route("unsubscribe", json!({"user": 1})).unwrap();
    assert!(url.starts_with("http://localhost/unsubscribe/1?signature="));
    let path = url.trim_start_matches("http://localhost");
    assert_eq!(get(path).await.content_string(), "valid");

    let tampered = path.replace("/1?", "/2?");
    assert_eq!(get(&tampered).await.content_string(), "invalid");
    assert_eq!(get("/unsubscribe/1").await.content_string(), "invalid");
}

#[tokio::test]
async fn signatures_match_laravels_algorithm() {
    use hmac::{Hmac, Mac};
    let _app = signing_app();
    Route::get("/unsubscribe/{user}", || async { "" }).name("unsubscribe");
    let url = URL::signed_route("unsubscribe", json!({"user": 1, "list": "news"})).unwrap();
    let (unsigned, signature) = url.split_once("&signature=").unwrap();
    assert_eq!(unsigned, "http://localhost/unsubscribe/1?list=news");
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(
        b"base64:c2VjcmV0LWtleS1zZWNyZXQta2V5LXNlY3JldC1rZXk=",
    )
    .unwrap();
    mac.update(unsigned.as_bytes());
    assert_eq!(signature, hex::encode(mac.finalize().into_bytes()));
}

#[tokio::test]
async fn temporary_signed_urls_expire() {
    let _app = signing_app();
    Route::get("/download/{file}", || async { "file" })
        .name("download")
        .middleware("signed");

    let url =
        URL::temporary_signed_route("download", std::time::Duration::from_secs(60), "report.pdf")
            .unwrap();
    assert!(url.contains("?expires="));
    let path = url.trim_start_matches("http://localhost").to_string();
    assert_eq!(get(&path).await.content_string(), "file");

    let past = Carbon::from_timestamp(Carbon::now().timestamp() - 60);
    let expired = URL::temporary_signed_route("download", past, "report.pdf").unwrap();
    let response = get(expired.trim_start_matches("http://localhost")).await;
    assert_eq!(response.status_code(), 403);
    let exception = response.exception().unwrap();
    assert!(
        exception
            .downcast_ref::<InvalidSignatureException>()
            .is_some()
    );
    assert_eq!(exception.to_string(), "Invalid signature.");
}

#[tokio::test]
async fn relative_and_ignored_signatures() {
    let _app = signing_app();
    Route::get("/relative/{id}", || async { "ok" })
        .name("relative")
        .middleware("signed:relative");
    Route::get("/paged/{id}", || async { "ok" })
        .name("paged")
        .middleware("signed:relative,page");

    let url = URL::signed_route_with("relative", 1, None::<std::time::Duration>, false).unwrap();
    assert!(url.starts_with("/relative/1?signature="));
    let request = request_with_headers(&url, "GET", &[("host", "elsewhere.test")]);
    assert_eq!(
        Route::router().dispatch(request).await.content_string(),
        "ok"
    );

    let url = URL::signed_route_with("paged", 1, None::<std::time::Duration>, false).unwrap();
    assert_eq!(get(&format!("{url}&page=3")).await.content_string(), "ok");
    assert_eq!(get(&format!("{url}&other=3")).await.status_code(), 403);
}

#[tokio::test]
async fn previous_keys_still_validate() {
    let (container, _guard) = signing_app();
    Route::get("/old/{id}", |request: Request| async move {
        URL::has_valid_signature(&request).to_string()
    })
    .name("old");
    configure(&container, json!({"app": {"key": "old-key"}}));
    let url = URL::signed_route("old", 1).unwrap();
    configure(
        &container,
        json!({"app": {"key": "new-key", "previous_keys": ["old-key"]}}),
    );
    assert_eq!(
        get(url.trim_start_matches("http://localhost"))
            .await
            .content_string(),
        "true"
    );
    configure(&container, json!({"app": {"key": "new-key"}}));
    assert_eq!(
        get(url.trim_start_matches("http://localhost"))
            .await
            .content_string(),
        "false"
    );
}

#[tokio::test]
async fn reserved_signature_parameters_are_rejected() {
    let _app = signing_app();
    Route::get("/x/{signature}", || async { "" }).name("x");
    assert!(URL::signed_route("x", json!({"signature": 1})).is_err());
    assert!(URL::signed_route("x", json!({"expires": 1})).is_err());
}

#[tokio::test]
async fn signing_requires_an_application_key() {
    let _app = app();
    Route::get("/x", || async { "" }).name("x");
    let error = URL::signed_route("x", ()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "No application encryption key has been specified."
    );
}

// ----------------------------------------------------------------------
// Redirects
// ----------------------------------------------------------------------

#[tokio::test]
async fn redirect_helpers() {
    let _app = app();
    Route::get("/profile/{id}", || async { "" }).name("profile");

    let response = redirect("/dashboard");
    assert_eq!(response.status_code(), 302);
    assert_eq!(response.target_url().unwrap(), "http://localhost/dashboard");

    assert_eq!(
        to_route("profile", 1).target_url().unwrap(),
        "http://localhost/profile/1"
    );
    assert_eq!(
        redirect_to_route("profile", 2).target_url().unwrap(),
        "http://localhost/profile/2"
    );
    assert_eq!(to_route("missing", ()).status_code(), 500);
    assert_eq!(
        Redirect::secure("/login").target_url().unwrap(),
        "https://localhost/login"
    );
    assert_eq!(Redirect::permanent("/new").status_code(), 301);
    assert_eq!(Redirect::temporary("/new").status_code(), 307);
    assert_eq!(
        Redirect::away("https://laravel.com").target_url().unwrap(),
        "https://laravel.com"
    );
    assert_eq!(Redirect::to_with_status("/x", 303).status_code(), 303);
    assert_eq!(
        Redirect::route_with_status("profile", 3, 301).status_code(),
        301
    );
}

#[tokio::test]
async fn back_uses_the_referer_or_the_session() {
    let _app = app();
    Route::post("/form", || async { back().with("status", "saved") });
    Route::post("/session", |request: Request| async move {
        request.set_attribute("_previous_url", "http://localhost/from-session");
        Redirect::back()
    });
    Route::post("/fallback", || async { Redirect::back_or("/home") });

    let request = request_with_headers(
        "/form",
        "POST",
        &[("referer", "http://localhost/form/create")],
    );
    let response = Route::router().dispatch(request).await;
    assert_eq!(
        response.target_url().unwrap(),
        "http://localhost/form/create"
    );
    assert_eq!(response.flashed()[0].0, "status");

    assert_eq!(
        send("/session", "POST").await.target_url().unwrap(),
        "http://localhost/from-session"
    );
    assert_eq!(
        send("/fallback", "POST").await.target_url().unwrap(),
        "http://localhost/home"
    );
    assert_eq!(
        send("/form", "POST").await.target_url().unwrap(),
        "http://localhost"
    );
}

#[tokio::test]
async fn refresh_and_previous_path() {
    let _app = app();
    Route::get("/current/page", || async { Redirect::refresh() });
    assert_eq!(
        get("/current/page?x=1").await.target_url().unwrap(),
        "http://localhost/current/page"
    );

    Route::get("/previous", || async { URL::previous_path() });
    let request = request_with_headers(
        "/previous",
        "GET",
        &[("referer", "http://localhost/users/1/?tab=a")],
    );
    assert_eq!(
        Route::router().dispatch(request).await.content_string(),
        "/users/1"
    );
}

// ----------------------------------------------------------------------
// The container
// ----------------------------------------------------------------------

#[tokio::test]
async fn the_provider_registers_shared_services() {
    let (container, _guard) = app();
    let router = container.make::<Router>();
    assert!(Arc::ptr_eq(&router, &Route::router()));
    router.get("/", || async { "Home" }).name("home");
    assert!(container.make::<UrlGenerator>().router().has("home"));
    assert!(router.has_middleware_alias("signed"));
}

#[tokio::test]
async fn facades_bootstrap_a_router_when_none_is_bound() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    Route::get("/", || async { "auto" }).name("home");
    assert!(container.bound::<Router>());
    assert_eq!(route("home", ()).unwrap(), "http://localhost");
}

#[tokio::test]
async fn routers_are_usable_as_kernel_destinations() {
    let _app = app();
    Route::get("/", || async { "kernel" });
    let destination = Route::router().as_destination();
    let response =
        illuminate_http::run_middleware(Request::create("/", "GET"), Vec::new(), destination).await;
    assert_eq!(response.content_string(), "kernel");
}

#[tokio::test]
async fn dispatch_works_on_multi_threaded_runtimes() {
    let router = Router::new();
    router.get("/users/{id}", |Path(id): Path<u64>| async move {
        format!("User {id}")
    });
    let handle =
        tokio::spawn(async move { router.dispatch(Request::create("/users/9", "GET")).await });
    assert_eq!(handle.await.unwrap().content_string(), "User 9");
}

#[tokio::test]
async fn requests_validate_their_own_signatures() {
    let _app = signing_app();
    Route::get("/invite/{team}", |request: Request| async move {
        format!(
            "{} {} {}",
            request.has_valid_signature(),
            request.has_valid_signature_while_ignoring(&["utm"]),
            request.current_route().unwrap().name().unwrap()
        )
    })
    .name("invite");
    let url = URL::signed_route("invite", 3).unwrap();
    let path = url.trim_start_matches("http://localhost");
    assert_eq!(get(path).await.content_string(), "true true invite");
    assert_eq!(
        get(&format!("{path}&utm=mail")).await.content_string(),
        "false true invite"
    );
}

// ----------------------------------------------------------------------
// Precognition
// ----------------------------------------------------------------------

/// What the foundation's `precognitive` middleware does to the request.
fn mark_precognitive() -> Arc<dyn Middleware> {
    middleware_fn(|request: Request, next: Next| async move {
        if request.is_attempting_precognition() {
            request.set_attribute("precognitive", true);
        }
        Ok(next.run(request).await)
    })
}

fn precognitive_request(uri: &str, method: &str) -> Request {
    request_with_headers(uri, method, &[("precognition", "true")])
}

#[tokio::test]
async fn precognitive_requests_resolve_arguments_but_skip_the_handler() {
    let _app = app();
    let ran = Arc::new(AtomicUsize::new(0));
    let extracted = Arc::new(Mutex::new(Vec::new()));

    let (ran_in_handler, seen) = (ran.clone(), extracted.clone());
    Route::post("/users/{id}", move |Path(id): Path<u64>, _request: Request| {
        seen.lock().unwrap().push(id);
        let ran = ran_in_handler.clone();
        async move {
            ran.fetch_add(1, Ordering::SeqCst);
            "Created"
        }
    })
    .middleware(mark_precognitive());

    let response = Route::router()
        .dispatch(precognitive_request("/users/7", "POST"))
        .await;
    assert_eq!(response.status_code(), 204);
    assert_eq!(response.header("precognition-success").as_deref(), Some("true"));
    assert!(response.content().is_empty());
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    // The handler wasn't even called: its body never ran.
    assert!(extracted.lock().unwrap().is_empty());

    // Without the header, the handler runs as usual.
    let response = send("/users/7", "POST").await;
    assert_eq!(response.content_string(), "Created");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert_eq!(*extracted.lock().unwrap(), vec![7]);
}

#[tokio::test]
async fn precognitive_requests_report_extraction_failures() {
    let _app = app();
    Route::get("/users/{id}", |Path(id): Path<u64>| async move { id.to_string() })
        .middleware(mark_precognitive());
    Route::post("/fail", |_request: Request| async {
        Err::<&str, _>(HttpException::new(500))
    })
    .middleware(middleware_fn(|request: Request, next: Next| async move {
        request.set_attribute("precognitive", true);
        Ok(next.run(request).await)
    }));
    Route::post("/forbidden", |_: Forbidden| async { "never" }).middleware(mark_precognitive());

    struct Forbidden;

    #[async_trait]
    impl FromRequest for Forbidden {
        async fn from_request(_request: &Request) -> Result<Self> {
            Err(HttpException::new(403).into())
        }
    }

    let response = Route::router()
        .dispatch(precognitive_request("/users/abc", "GET"))
        .await;
    assert_eq!(response.status_code(), 404);
    assert!(response.header("precognition-success").is_none());

    let response = Route::router()
        .dispatch(precognitive_request("/forbidden", "POST"))
        .await;
    assert_eq!(response.status_code(), 403);

    // The handler itself never runs, so its errors can't happen.
    assert_eq!(send("/fail", "POST").await.status_code(), 204);
}

#[tokio::test]
async fn controller_methods_are_precognitive_too() {
    let _app = app();
    Route::get("/users/{id}", UserController::show).middleware(mark_precognitive());

    let response = Route::router()
        .dispatch(precognitive_request("/users/3", "GET"))
        .await;
    assert_eq!(response.status_code(), 204);
    assert_eq!(get("/users/3").await.content_string(), "User 3");
}

#[tokio::test]
async fn the_header_alone_does_not_make_a_request_precognitive() {
    let _app = app();
    Route::post("/users", |request: Request| async move {
        format!("precognitive: {}", request.is_precognitive())
    });

    let response = Route::router()
        .dispatch(precognitive_request("/users", "POST"))
        .await;
    assert_eq!(response.content_string(), "precognitive: false");
}

#[tokio::test]
async fn missing_handlers_still_respond_to_precognitive_requests() {
    let _app = app();
    struct Location;

    #[async_trait]
    impl FromRequest for Location {
        async fn from_request(request: &Request) -> Result<Self> {
            match route_parameter_for(request, "location") {
                Some((value, _)) if value == "home" => Ok(Location),
                _ => Err(HttpException::new(404).into()),
            }
        }
    }

    Route::put("/locations/{location}", |_location: Location| async { "updated" })
        .middleware(mark_precognitive())
        .missing(|| async { Redirect::to("/locations") });

    let response = Route::router()
        .dispatch(precognitive_request("/locations/home", "PUT"))
        .await;
    assert_eq!(response.status_code(), 204);

    let response = Route::router()
        .dispatch(precognitive_request("/locations/mars", "PUT"))
        .await;
    assert!(response.is_redirect());
    assert_eq!(response.target_url().unwrap(), "http://localhost/locations");
}

#[tokio::test]
async fn redirect_view_and_resource_routes_are_precognitive() {
    let _app = app();
    let precognitive = |route: RouteDefinition| route.middleware(mark_precognitive());
    precognitive(Route::redirect("/here", "/there"));
    precognitive(Route::view("/welcome", "welcome", json!({})));
    Route::middleware(mark_precognitive()).group(|| {
        Route::resource("photos", PhotoController).only(&["show"]);
    });

    for uri in ["/here", "/welcome", "/photos/1"] {
        let response = Route::router().dispatch(precognitive_request(uri, "GET")).await;
        assert_eq!(response.status_code(), 204, "{uri}");
        assert_eq!(response.header("precognition-success").as_deref(), Some("true"));
    }

    assert_eq!(get("/here").await.status_code(), 302);
    assert_eq!(get("/photos/1").await.content_string(), "show 1");
}
