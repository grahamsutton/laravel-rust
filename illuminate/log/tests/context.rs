use std::sync::Arc;

use illuminate_container::Container;
use illuminate_http::{Request, with_request};
use illuminate_log::{Context, Level, Logger, Monolog, TestHandler};
use illuminate_support::json;

fn logger() -> (Logger, Arc<TestHandler>) {
    let handler = Arc::new(TestHandler::new());
    (Logger::new(Monolog::new("testing").with_handler_arc(handler.clone())), handler)
}

#[tokio::test]
async fn context_is_added_to_every_log_entry() {
    let _guard = Container::set_local_instance(Arc::new(Container::new()));
    let (logger, handler) = logger();

    Context::add("trace_id", "8f5b0a");
    Context::add_hidden("api_key", "secret");
    logger.info("User signed in.");

    let record = &handler.records()[0];
    assert_eq!(record.extra.get("trace_id"), Some(&json!("8f5b0a")));
    assert!(!record.extra.contains_key("api_key"), "hidden context is never logged");
}

#[tokio::test]
async fn context_methods_behave_like_laravel() {
    Context::run(async {
        Context::add("user", 1);
        Context::add_if("user", 2);
        assert_eq!(Context::get("user"), Some(json!(1)));
        assert!(Context::has("user") && Context::missing("team"));

        Context::push("breadcrumbs", ["home", "settings"]);
        assert!(Context::stack_contains("breadcrumbs", "settings"));
        assert_eq!(Context::pop("breadcrumbs"), Some(json!("settings")));

        assert_eq!(Context::increment("visits", 2), 2);
        assert_eq!(Context::decrement("visits", 1), 1);

        assert_eq!(Context::only(&["user"]).len(), 1);
        assert!(!Context::except(&["user"]).contains_key("user"));
        assert_eq!(Context::remember("locale", || json!("en")), json!("en"));

        let inner = Context::scope(async { Context::get("job") }, json!({"job": "import"}), json!({})).await;
        assert_eq!(inner, Some(json!("import")));
        assert!(Context::missing("job"), "scoped data is removed afterwards");

        let dehydrated = Context::dehydrate().unwrap();
        Context::flush();
        assert!(Context::is_empty());
        Context::hydrate(&dehydrated);
        assert_eq!(Context::get("user"), Some(json!(1)));

        assert_eq!(Context::pull("user"), Some(json!(1)));
        assert!(Context::missing("user"));
    })
    .await;
}

#[tokio::test]
async fn concurrent_requests_keep_their_own_context() {
    let _guard = Container::set_local_instance(Arc::new(Container::new()));
    let (logger, handler) = logger();
    let logger = Arc::new(logger);

    let requests = (0..8).map(|id| {
        let logger = logger.clone();
        with_request(Request::create("/", "GET"), async move {
            Context::add("request_id", id);
            logger.with_context(json!({"user": id}));
            tokio::task::yield_now().await;
            logger.info(format!("Handled request {id}."));
        })
    });
    futures::future::join_all(requests).await;

    let records = handler.records();
    assert_eq!(records.len(), 8);
    for record in records {
        let id = record.message.trim_start_matches("Handled request ").trim_end_matches('.');
        assert_eq!(record.extra["request_id"].to_string(), id);
        assert_eq!(record.context["user"].to_string(), id);
    }
    assert!(Context::all().is_empty(), "request context doesn't leak into the application's");
    assert!(handler.has_record(Level::Info, "Handled request 0."));
}
