//! Session blocking, scheme restrictions, metadata, handler swapping and
//! bulk resource registration.

use std::sync::Arc;

use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_http::{Request, Response, with_request};
use illuminate_routing::{
    CurrentRoute, ResourceController, Route, RoutingServiceProvider, async_trait, route,
};
use illuminate_support::{Result, json};

fn app() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    RoutingServiceProvider.register(&container);
    (container, guard)
}

async fn send(uri: &str, method: &str) -> Response {
    Route::router().dispatch(Request::create(uri, method)).await
}

struct PhotoController;

#[async_trait]
impl ResourceController for PhotoController {
    async fn show(&self, _request: Request) -> Result<Response> {
        Ok(Response::new("Photo"))
    }
}

struct ProfileController;

#[async_trait]
impl ResourceController for ProfileController {
    async fn show(&self, _request: Request) -> Result<Response> {
        Ok(Response::new("Profile"))
    }
}

#[tokio::test]
async fn routes_can_block_concurrent_session_requests() {
    let _app = app();
    let blocking = Route::post("/profile", || async { "Saved" }).block(10, 5);
    let plain = Route::post("/order", || async { "Ordered" });

    assert_eq!(blocking.locks_for(), Some(10));
    assert_eq!(blocking.waits_for(), Some(5));
    assert_eq!(plain.locks_for(), None);
    assert_eq!(plain.waits_for(), None);

    let blocking = blocking.without_blocking();
    assert_eq!(blocking.locks_for(), None);
    assert_eq!(blocking.waits_for(), None);
}

#[tokio::test]
async fn routes_can_be_restricted_to_a_scheme() {
    let _app = app();
    Route::get("/checkout", || async { "Secure checkout" })
        .https()
        .name("checkout");
    Route::get("/legacy", || async { "Plain" })
        .http()
        .name("legacy");
    Route::get("/any", || async { "Any" });

    let checkout = Route::get_by_name("checkout").unwrap();
    assert!(checkout.https_only());
    assert!(checkout.secure());
    assert!(!checkout.http_only());
    assert!(Route::get_by_name("legacy").unwrap().http_only());

    assert_eq!(
        send("http://localhost/checkout", "GET").await.status_code(),
        404
    );
    assert_eq!(
        send("https://localhost/checkout", "GET")
            .await
            .content_string(),
        "Secure checkout"
    );
    assert_eq!(
        send("https://localhost/legacy", "GET").await.status_code(),
        404
    );
    assert_eq!(
        send("http://localhost/legacy", "GET")
            .await
            .content_string(),
        "Plain"
    );
    assert_eq!(
        send("https://localhost/any", "GET").await.content_string(),
        "Any"
    );

    // Generated URLs use the route's scheme.
    assert_eq!(route("checkout", ()).unwrap(), "https://localhost/checkout");
    assert_eq!(route("legacy", ()).unwrap(), "http://localhost/legacy");
}

#[tokio::test]
async fn routes_and_groups_carry_metadata() {
    let _app = app();
    Route::metadata(json!({"docs": {"tag": "admin", "visible": true}, "roles": ["admin"]}))
        .prefix("admin")
        .group(|| {
            Route::metadata(json!({"docs": {"section": "users"}})).group(|| {
                Route::get("/users", || async { "Users" })
                    .name("admin.users")
                    .metadata(json!({"docs": {"visible": false}, "roles": ["owner"]}));
            });
            Route::get("/plain", || async { "Plain" }).name("admin.plain");
        });
    Route::get("/bare", || async { "Bare" }).name("bare");

    let users = Route::get_by_name("admin.users").unwrap();
    assert_eq!(
        users.get_metadata(),
        json!({"docs": {"tag": "admin", "visible": false, "section": "users"}, "roles": ["owner"]})
    );
    assert_eq!(users.get_metadata_key("docs.section"), json!("users"));
    assert_eq!(
        Route::get_by_name("admin.plain").unwrap().get_metadata(),
        json!({"docs": {"tag": "admin", "visible": true}, "roles": ["admin"]})
    );

    let bare = Route::get_by_name("bare").unwrap();
    assert_eq!(bare.get_metadata(), json!({}));
    assert_eq!(bare.get_metadata_or("docs.tag", "none"), json!("none"));
    let bare = bare.metadata(json!({"a": 1})).set_metadata(json!({"b": 2}));
    assert_eq!(bare.get_metadata(), json!({"b": 2}));

    // Handlers can read the matched route's metadata.
    Route::get("/meta", |request: Request| async move {
        let current = request.extension::<CurrentRoute>().unwrap();
        current
            .route()
            .get_metadata_key("feature")
            .as_str()
            .unwrap_or_default()
            .to_string()
    })
    .metadata(json!({"feature": "beta"}));
    assert_eq!(send("/meta", "GET").await.content_string(), "beta");
}

#[tokio::test]
async fn handlers_can_be_swapped_and_matched_by_action() {
    let _app = app();
    Route::get("/greeting", || async { "Hello" }).uses(|| async { "Howdy" });
    Route::resource("photos", PhotoController);

    assert_eq!(send("/greeting", "GET").await.content_string(), "Howdy");

    let request = Request::create("/photos/1", "GET");
    let router = Route::router();
    let (route, parameters) = router.find_route(&request).unwrap();
    let response = with_request(request.clone(), async {
        let response = router.run_route(request.clone(), route, parameters).await;
        assert!(Route::uses(&["PhotoController@*"]));
        assert!(Route::uses(&["Other@*", "*@show"]));
        assert!(!Route::uses(&["ProfileController@*"]));
        assert!(Route::current_route_uses("PhotoController@show"));
        assert!(!Route::current_route_uses("PhotoController"));
        response
    })
    .await;
    assert_eq!(response.content_string(), "Photo");

    // Outside of a request nothing is current.
    assert!(!Route::uses(&["*"]));
}

#[tokio::test]
async fn many_singletons_and_soft_deletable_resources_register_at_once() {
    let _app = app();
    Route::singletons(vec![(
        "profile",
        Arc::new(ProfileController) as Arc<dyn ResourceController>,
    )]);
    Route::api_singletons(vec![(
        "settings",
        Arc::new(ProfileController) as Arc<dyn ResourceController>,
    )]);
    Route::soft_deletable_resources(vec![(
        "photos",
        Arc::new(PhotoController) as Arc<dyn ResourceController>,
    )]);

    assert!(Route::has_all(&[
        "profile.show",
        "profile.edit",
        "profile.update"
    ]));
    assert!(!Route::has("profile.create"));
    assert!(Route::has_all(&["settings.show", "settings.update"]));
    assert!(!Route::has("settings.edit"));
    assert_eq!(send("/profile", "GET").await.content_string(), "Profile");

    for name in ["photos.show", "photos.edit", "photos.update"] {
        assert!(
            Route::get_by_name(name).unwrap().allows_trashed_bindings(),
            "{name}"
        );
    }
    for name in ["photos.index", "photos.store", "photos.destroy"] {
        assert!(
            !Route::get_by_name(name).unwrap().allows_trashed_bindings(),
            "{name}"
        );
    }
}
