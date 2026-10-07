//! The testing API: fakes, sequences, recording and assertions.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::fresh_app;
use illuminate_container::{Container, ServiceProvider};
use illuminate_events::{Dispatcher, Event};
use illuminate_http_client::{
    ConnectionException, ConnectionFailed, Factory, FakeResponse, Http, HttpClientServiceProvider,
    PendingRequest, Request, RequestException, RequestSending, Response, ResponseFuture,
    ResponseReceived, StrayRequestException,
};
use illuminate_support::{Conditionable, Sleep, Value, json};

#[tokio::test]
async fn fake_stubs_every_request() {
    let _app = fresh_app();
    Http::fake();
    Http::assert_nothing_sent();

    let response = Http::post("https://example.com/users", json!({"name": "Taylor"}))
        .await
        .unwrap();

    assert!(response.ok());
    assert!(response.body().is_empty());
    Http::assert_sent_count(1);
    Http::assert_sent(|request| request.method() == "POST" && request["name"] == "Taylor");
}

#[tokio::test]
async fn fake_urls_match_patterns_in_order() {
    let _app = fresh_app();
    Http::fake_urls([
        (
            "github.com/*",
            Http::response(json!({"foo": "bar"}), 200, &[("X-Custom", "yes")]),
        ),
        ("google.com/*", Http::response("Hello World", 200, &[])),
        ("chatgpt.com/*", FakeResponse::from(204)),
        ("laravel.com/docs", Http::response(Value::Null, 500, &[])),
        ("*", Http::response("fallback", 200, &[])),
    ]);

    let github = Http::get("https://github.com/laravel/framework")
        .await
        .unwrap();
    assert_eq!(github["foo"], "bar");
    assert_eq!(github.header("X-Custom"), "yes");
    assert_eq!(github.header("Content-Type"), "application/json");

    assert_eq!(
        Http::get("https://google.com/search").await.unwrap().body(),
        "Hello World"
    );
    assert!(
        Http::get("https://chatgpt.com/")
            .await
            .unwrap()
            .no_content()
    );
    assert!(
        Http::get("https://laravel.com/docs")
            .await
            .unwrap()
            .server_error()
    );
    assert_eq!(
        Http::get("https://forge.laravel.com").await.unwrap().body(),
        "fallback"
    );

    Http::assert_sent_count(5);
    Http::assert_sent_in_order(&[
        "https://github.com/laravel/framework",
        "https://google.com/search",
        "https://chatgpt.com/",
        "https://laravel.com/docs",
        "https://forge.laravel.com",
    ]);
}

#[tokio::test]
async fn string_json_and_status_shorthands_work() {
    let _app = fresh_app();
    Http::fake_urls([("google.com/*", "Hello World")]);
    Http::fake_urls([("github.com/*", json!({"foo": "bar"}))]);
    Http::fake_urls([("chatgpt.com/*", 418)]);

    assert_eq!(
        Http::get("https://google.com/a").await.unwrap().body(),
        "Hello World"
    );
    assert_eq!(
        Http::get("https://github.com/a").await.unwrap()["foo"],
        "bar"
    );
    assert_eq!(
        Http::get("https://chatgpt.com/a").await.unwrap().status(),
        418
    );

    let error = {
        Http::stub_url("bad.test/*", 999);
        Http::get("https://bad.test/a").await.unwrap_err()
    };
    assert_eq!(
        error.to_string(),
        "HTTP status code must be between 100 and 599."
    );
}

#[tokio::test]
async fn fake_callbacks_inspect_the_request() {
    let _app = fresh_app();
    Http::fake_using(|request: &Request| {
        if request.url().contains("teapot") {
            Http::response("I'm a teapot", 418, &[])
        } else {
            Http::response(json!({"you_sent": request.data()}), 200, &[])
        }
    });

    assert_eq!(
        Http::get("https://example.com/teapot")
            .await
            .unwrap()
            .status(),
        418
    );

    let response = Http::put("https://example.com/users/1", json!({"name": "Taylor"}))
        .await
        .unwrap();
    assert_eq!(response.json_path("you_sent.name"), "Taylor");
}

#[tokio::test]
async fn callbacks_returning_none_fall_through() {
    let _app = fresh_app();
    Http::prevent_stray_requests();
    Http::fake_using(|request: &Request| {
        request
            .url()
            .contains("first")
            .then(|| FakeResponse::from("first"))
    });
    Http::fake_urls([("*", "second")]);

    assert_eq!(
        Http::get("https://a.test/first").await.unwrap().body(),
        "first"
    );
    assert_eq!(
        Http::get("https://a.test/other").await.unwrap().body(),
        "second"
    );
}

#[tokio::test]
async fn sequences_return_responses_in_order() {
    let _app = fresh_app();
    Http::fake_urls([(
        "github.com/*",
        Http::sequence()
            .push("Hello World", 200)
            .push(json!({"foo": "bar"}), 200)
            .push_status(404),
    )]);

    assert_eq!(
        Http::get("https://github.com/1").await.unwrap().body(),
        "Hello World"
    );
    assert_eq!(
        Http::get("https://github.com/2").await.unwrap()["foo"],
        "bar"
    );
    assert!(Http::get("https://github.com/3").await.unwrap().not_found());
    Http::assert_sequences_are_empty();

    let error = Http::get("https://github.com/4").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "A request was made, but the response sequence is empty."
    );
}

#[tokio::test]
async fn fake_sequences_can_have_a_default() {
    let _app = fresh_app();
    let sequence = Http::fake_sequence("*")
        .push("first", 200)
        .when_empty(Http::response("empty", 200, &[]));

    assert_eq!(Http::get("https://a.test").await.unwrap().body(), "first");
    assert!(sequence.is_empty());
    assert_eq!(Http::get("https://a.test").await.unwrap().body(), "empty");
    assert_eq!(Http::get("https://a.test").await.unwrap().body(), "empty");

    let sequence = Http::sequence_of(["a", "b"]);
    assert_eq!(sequence.len(), 2);
}

#[tokio::test]
#[should_panic(expected = "Not all response sequences are empty.")]
async fn unused_sequences_fail_the_assertion() {
    let _app = fresh_app();
    Http::fake_sequence("*").push("unused", 200);
    Http::assert_sequences_are_empty();
}

#[tokio::test]
async fn failed_connections_are_connection_exceptions() {
    let _app = fresh_app();
    Http::fake_urls([
        ("github.com/*", Http::failed_connection()),
        (
            "laravel.com/*",
            Http::failed_connection_with("Laravel is down"),
        ),
    ]);

    let error = Http::get("https://github.com/laravel").await.unwrap_err();
    assert!(error.is::<ConnectionException>());
    assert_eq!(
        error.to_string(),
        "Could not resolve host: github.com for https://github.com/laravel."
    );

    let error = Http::get("https://laravel.com/docs").await.unwrap_err();
    assert_eq!(error.to_string(), "Laravel is down");

    let recorded = Http::recorded();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[0].1.is_none());
}

#[tokio::test]
async fn failed_requests_can_be_stubbed() {
    let exception = Http::failed_request(json!({"code": "not_found"}), 404, &[]);
    assert_eq!(exception.response.status(), 404);
    assert_eq!(exception.response["code"], "not_found");
    assert!(
        exception
            .to_string()
            .starts_with("HTTP request returned status code 404")
    );
}

#[tokio::test]
async fn stray_requests_can_be_prevented() {
    let _app = fresh_app();
    Http::prevent_stray_requests();
    assert!(Http::preventing_stray_requests());
    Http::allow_stray_requests(["http://127.0.0.1:1/allowed*"]);
    Http::fake_urls([("github.com/*", Http::response("ok", 200, &[]))]);

    assert_eq!(
        Http::get("https://github.com/laravel/framework")
            .await
            .unwrap()
            .body(),
        "ok"
    );

    let error = Http::get("https://laravel.com").await.unwrap_err();
    let exception = error.downcast_ref::<StrayRequestException>().unwrap();
    assert_eq!(exception.uri, "https://laravel.com");
    assert_eq!(
        error.to_string(),
        "Attempted request to [https://laravel.com] without a matching fake."
    );

    // Allowed URLs reach the network (and fail to connect, since nothing listens there).
    let error = Http::get("http://127.0.0.1:1/allowed").await.unwrap_err();
    assert!(error.is::<ConnectionException>());

    Http::factory().prevent_stray_requests(false);
    assert!(!Http::preventing_stray_requests());
}

#[tokio::test]
async fn requests_can_be_inspected() {
    let _app = fresh_app();
    Http::fake();

    Http::base_url("https://api.example.com")
        .with_headers([("X-First", "foo")])
        .with_attributes(json!({"tenant": "laravel"}))
        .get_with("/users", json!({"name": "Taylor", "page": 2}))
        .await
        .unwrap();

    Http::as_form()
        .post("https://example.com/form", json!({"name": "Sara"}))
        .await
        .unwrap();
    Http::attach("photo", "PNG", "photo.png")
        .post("https://example.com/upload", ())
        .await
        .unwrap();

    Http::assert_sent(|request| {
        request.url() == "https://api.example.com/users?name=Taylor&page=2"
            && request.has_header_value("X-First", "foo")
            && request.has_headers(&[("X-First", "foo")])
            && request["name"] == "Taylor"
            && request["page"] == "2"
            && request.attributes()["tenant"] == "laravel"
            && !request.is_json()
    });
    Http::assert_sent(|request| {
        request.is_form() && request["name"] == "Sara" && request.body() == "name=Sara"
    });
    Http::assert_sent(|request| {
        request.is_multipart()
            && request.has_file("photo")
            && request.has_file_with("photo", Some("PNG"), Some("photo.png"))
    });
    Http::assert_not_sent(|request| request.url() == "https://example.com/posts");

    let successful = Http::recorded_fn(|request, response| {
        request.url().contains("form") && response.is_some_and(Response::successful)
    });
    assert_eq!(successful.len(), 1);
}

#[tokio::test]
#[should_panic(expected = "An expected request was not recorded.")]
async fn assert_sent_fails_without_a_match() {
    let _app = fresh_app();
    Http::fake();
    Http::get("https://laravel.com").await.unwrap();
    Http::assert_sent(|request| request.url() == "https://forge.laravel.com");
}

#[tokio::test]
#[should_panic(expected = "Unexpected request was recorded.")]
async fn assert_not_sent_fails_with_a_match() {
    let _app = fresh_app();
    Http::fake();
    Http::get("https://laravel.com").await.unwrap();
    Http::assert_not_sent(|request| request.url() == "https://laravel.com");
}

#[tokio::test]
#[should_panic(expected = "Requests were recorded.")]
async fn assert_nothing_sent_fails_after_a_request() {
    let _app = fresh_app();
    Http::fake();
    Http::get("https://laravel.com").await.unwrap();
    Http::assert_nothing_sent();
}

#[tokio::test]
#[should_panic(expected = "An expected request (#2) was not recorded.")]
async fn assert_sent_in_order_checks_the_order() {
    let _app = fresh_app();
    Http::fake();
    Http::get("https://a.test").await.unwrap();
    Http::get("https://b.test").await.unwrap();
    Http::assert_sent_in_order(&["https://a.test", "https://c.test"]);
}

#[tokio::test]
async fn fakes_are_isolated_per_container() {
    let factory = {
        let _app = fresh_app();
        Http::fake();
        Http::get("https://laravel.com").await.unwrap();
        Http::assert_sent_count(1);
        Http::factory()
    };

    let _app = fresh_app();
    assert!(!Arc::ptr_eq(&factory, &Http::factory()));
    Http::fake();
    Http::assert_nothing_sent();
}

#[tokio::test]
async fn the_service_provider_registers_the_factory() {
    let (container, _guard) = fresh_app();
    HttpClientServiceProvider.register(&container);

    let factory = container.make::<Factory>();
    assert!(Arc::ptr_eq(&factory, &Http::factory()));
    assert!(container.bound::<Factory>());
}

#[tokio::test]
async fn events_are_dispatched() {
    let (container, _guard) = fresh_app();
    container.instance(Dispatcher::new());
    Event::fake();

    Http::fake_urls([
        ("laravel.com/*", Http::failed_connection()),
        ("*", Http::response("ok", 200, &[])),
    ]);

    Http::get("https://example.com").await.unwrap();
    let _ = Http::get("https://laravel.com/docs").await;

    Event::assert_dispatched_times::<RequestSending>(2);
    Event::assert_dispatched_with(|event: &ResponseReceived| {
        event.request.url() == "https://example.com" && event.response.body() == "ok"
    });
    Event::assert_dispatched_with(|event: &ConnectionFailed| {
        event.request.url() == "https://laravel.com/docs"
            && event.exception.message().contains("laravel.com")
    });
}

#[tokio::test]
async fn events_go_to_an_explicit_dispatcher() {
    let (container, _guard) = fresh_app();
    let dispatcher = Arc::new(Dispatcher::new());
    let sent = Arc::new(AtomicUsize::new(0));
    let counter = sent.clone();
    dispatcher.listen(move |_: &RequestSending| {
        counter.fetch_add(1, Ordering::SeqCst);
        async {}
    });
    container.instance(Factory::with_dispatcher(dispatcher));

    Http::fake();
    Http::get("https://laravel.com").await.unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn middleware_sees_and_changes_requests_and_responses() {
    let _app = fresh_app();
    Http::fake_urls([("*", "body")]);

    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();

    let response =
        Http::with_request_middleware(|request| request.with_header("X-Example", "Value"))
            .with_response_middleware(|response| response.with_header("X-Finished", "yes"))
            .with_middleware(move |request, next| {
                let seen = seen.clone();
                async move {
                    seen.lock()
                        .unwrap()
                        .push(request.header("X-Example").join(""));
                    next.run(request)
                        .await
                        .map(|response| response.with_body("changed"))
                }
            })
            .get("https://example.com")
            .await
            .unwrap();

    assert_eq!(response.header("X-Finished"), "yes");
    assert_eq!(response.body(), "changed");
    assert_eq!(*log.lock().unwrap(), ["Value"]);

    // The recorder sits inside the middleware: it sees the modified
    // request and the unmodified response.
    let recorded = Http::recorded();
    assert!(recorded[0].0.has_header_value("X-Example", "Value"));
    assert_eq!(recorded[0].1.as_ref().unwrap().body(), "body");
}

#[tokio::test]
async fn middleware_may_short_circuit() {
    let _app = fresh_app();
    Http::fake();

    let response = Http::with_middleware(|_request, _next| async {
        Ok(Response::new(203, Default::default(), "cached"))
    })
    .get("https://example.com")
    .await
    .unwrap();

    assert_eq!(response.status(), 203);
    Http::assert_nothing_sent();
}

#[tokio::test]
async fn global_configuration_applies_to_every_request() {
    let _app = fresh_app();
    Http::fake();
    Http::global_options(json!({"headers": {"X-Global": "1"}, "timeout": 5}));
    Http::global_request_middleware(|request| {
        request.with_header("User-Agent", "Example Application/1.0")
    });
    Http::global_response_middleware(|response| response.with_header("X-Finished-At", "now"));
    Http::global_middleware(|request, next| async move {
        next.run(request.with_header("X-Wrapped", "1")).await
    });

    let pending = Http::new_request();
    assert_eq!(pending.get_options()["timeout"], 5);

    let response = pending.get("https://example.com").await.unwrap();
    assert_eq!(response.header("X-Finished-At"), "now");
    Http::assert_sent(|request| {
        request.has_header_value("X-Global", "1")
            && request.has_header_value("User-Agent", "Example Application/1.0")
            && request.has_header_value("X-Wrapped", "1")
    });

    let plain = Http::without_global_configuration(Http::new_request);
    assert_eq!(plain.get_options()["timeout"], 30);
    plain.get("https://plain.test").await.unwrap();
    Http::assert_sent(|request| {
        request.url() == "https://plain.test" && !request.has_header("X-Global")
    });
    assert_eq!(Http::new_request().get_options()["timeout"], 5);
}

#[tokio::test]
async fn before_sending_and_after_response_callbacks_run() {
    let _app = fresh_app();
    Http::fake_urls([("*", "original")]);

    let response = Http::before_sending(|request| {
        *request = request.clone().with_header("X-Before", "1");
    })
    .after_response(|response, request| {
        assert!(request.has_header("X-Before"));
        response.with_body("replaced")
    })
    .get("https://example.com")
    .await
    .unwrap();

    assert_eq!(response.body(), "replaced");
    Http::assert_sent(|request| request.has_header_value("X-Before", "1"));
}

#[tokio::test]
async fn throwing_on_errors() {
    let _app = fresh_app();
    Http::fake_urls([
        ("*/missing", Http::response("Not Found", 404, &[])),
        ("*", Http::response("ok", 200, &[])),
    ]);

    assert!(Http::throw().get("https://a.test/ok").await.is_ok());

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let error = Http::throw_with(move |response, exception| {
        assert_eq!(response.status(), 404);
        assert_eq!(exception.code(), 404);
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .get("https://a.test/missing")
    .await
    .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        error.to_string(),
        "HTTP request returned status code 404:\nNot Found\n"
    );

    assert!(
        Http::throw_if(true)
            .get("https://a.test/missing")
            .await
            .is_err()
    );
    assert!(
        Http::throw_unless(true)
            .get("https://a.test/missing")
            .await
            .is_ok()
    );
    assert!(
        Http::throw_if_fn(|r| r.status() == 404)
            .get("https://a.test/missing")
            .await
            .is_err()
    );
    assert!(
        Http::throw_if_fn(|r| r.status() == 500)
            .get("https://a.test/missing")
            .await
            .is_ok()
    );

    let error = Http::throw()
        .truncate_exceptions_at(3)
        .get("https://a.test/missing")
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "HTTP request returned status code 404:\nNot (truncated...)\n"
    );

    let error = Http::throw()
        .dont_truncate_exceptions()
        .get("https://a.test/missing")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("HTTP/1.1 404 Not Found"));
}

#[tokio::test]
async fn retries_use_fakes_and_sleep() {
    let _app = fresh_app();
    Sleep::fake();
    Http::fake_sequence("a.test/*")
        .push_status(500)
        .push_failed_connection()
        .push("recovered", 200);

    let response = Http::retry(3, 50).get("https://a.test/x").await.unwrap();
    assert_eq!(response.body(), "recovered");
    Http::assert_sent_count(3);
    Sleep::assert_sequence(vec![
        Sleep::for_(50).milliseconds(),
        Sleep::for_(50).milliseconds(),
    ]);

    Http::fake_sequence("b.test/*")
        .push_status(503)
        .push_status(503);
    let error = Http::retry_using(2, |attempt, error| {
        assert!(error.is::<RequestException>());
        attempt as u64 * 1000
    })
    .get("https://b.test/x")
    .await
    .unwrap_err();
    assert!(error.is::<RequestException>());
    Sleep::assert_slept(|duration| duration.as_millis() == 1000, 1);
    Sleep::stop_faking();
}

#[tokio::test]
async fn retry_when_decides_and_may_modify_the_request() {
    let _app = fresh_app();
    Http::fake_sequence("*")
        .push_status(401)
        .push_status(500)
        .push("never", 200);

    let response = Http::with_token("expired", "Bearer")
        .retry(5, 0)
        .retry_when(|error, request| {
            let status = error
                .downcast_ref::<RequestException>()
                .map(|e| e.response.status());
            if status == Some(401) {
                *request = request.clone().with_token("fresh", "Bearer");
                return true;
            }
            false
        })
        .retry_throw(false)
        .get("https://a.test")
        .await
        .unwrap();

    assert_eq!(response.status(), 500);
    Http::assert_sent_count(2);
    Http::assert_sent(|request| request.has_header_value("Authorization", "Bearer fresh"));
}

#[tokio::test]
async fn faked_redirects_are_followed() {
    let _app = fresh_app();
    Http::fake_urls([
        (
            "a.test/old",
            Http::response(Value::Null, 301, &[("Location", "/new")]),
        ),
        ("a.test/new", Http::response("moved", 200, &[])),
    ]);

    let response = Http::get("https://a.test/old").await.unwrap();
    assert_eq!(response.body(), "moved");
    assert_eq!(response.effective_uri(), Some("https://a.test/new"));
    Http::assert_sent_in_order(&["https://a.test/old", "https://a.test/new"]);

    let response = Http::without_redirecting()
        .get("https://a.test/old")
        .await
        .unwrap();
    assert!(response.moved_permanently());
}

#[tokio::test]
async fn pools_and_batches_use_fakes() {
    let _app = fresh_app();
    Http::fake_urls([
        ("*/down", Http::failed_connection()),
        ("*/missing", FakeResponse::from(404)),
        ("*", Http::response("ok", 200, &[])),
    ]);

    let responses = Http::pool_with_concurrency(
        |pool| {
            vec![
                pool.get("https://a.test/first"),
                pool.as_("down").get("https://a.test/down"),
                pool.with_headers([("X-Pooled", "1")])
                    .post("https://a.test/second", json!({"n": 2})),
            ]
        },
        1,
    )
    .await;

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0].body(), "ok");
    assert!(
        responses
            .named("down")
            .unwrap()
            .as_ref()
            .unwrap_err()
            .is::<ConnectionException>()
    );
    assert!(responses.get(1).unwrap().is_ok());
    assert!(format!("{responses:?}").contains("down"));
    Http::assert_sent(|request| request.has_header_value("X-Pooled", "1") && request["n"] == 2);

    let progress = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(Mutex::new(Vec::new()));
    let finished = Arc::new(AtomicUsize::new(0));
    let (p, f, done) = (progress.clone(), failures.clone(), finished.clone());

    let responses = Http::batch(|batch| {
        vec![
            batch.get("https://a.test/1"),
            batch.as_("missing").get("https://a.test/missing"),
            batch.get("https://a.test/down"),
        ]
    })
    .before(|batch| assert_eq!(batch.pending_requests, 3))
    .progress(move |_, _, response| {
        assert!(response.ok());
        p.fetch_add(1, Ordering::SeqCst);
    })
    .catch(move |_, key, _| f.lock().unwrap().push(key.to_string()))
    .then(|_, _| panic!("the batch has failures"))
    .finally(move |batch, results| {
        assert_eq!(batch.processed_requests(), 3);
        assert_eq!(batch.failed_requests, 2);
        assert!(batch.has_failures() && !batch.finished());
        assert_eq!(results.len(), 3);
        done.fetch_add(1, Ordering::SeqCst);
    })
    .send()
    .await;

    assert_eq!(progress.load(Ordering::SeqCst), 1);
    let mut failed = failures.lock().unwrap().clone();
    failed.sort();
    assert_eq!(failed, ["1", "missing"]);
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert!(responses["missing"].not_found());
}

#[tokio::test]
async fn sinks_receive_faked_bodies() {
    let _app = fresh_app();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fake.txt");
    Http::fake_urls([("*", "faked contents")]);

    Http::sink(&path).get("https://a.test/file").await.unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), "faked contents");
}

#[tokio::test]
async fn pending_requests_are_conditionable_and_reusable() {
    let _app = fresh_app();
    Http::fake();

    let client: PendingRequest = Http::base_url("https://api.test")
        .when(true, |request| request.with_token("secret", "Bearer"))
        .when(false, |request| request.with_header("X-Never", "1"));

    client.clone().get("/a").await.unwrap();
    client.get("/b").await.unwrap();

    Http::assert_sent_count(2);
    Http::assert_not_sent(|request| request.has_header("X-Never"));
    Http::assert_sent(|request| {
        request.url() == "https://api.test/b" && request.has_header("Authorization")
    });
}

#[tokio::test]
async fn data_serialization_errors_are_returned() {
    let _app = fresh_app();
    Http::fake();

    let mut invalid = std::collections::HashMap::new();
    invalid.insert(vec![1u8], "keys must be strings");
    assert!(Http::post("https://a.test", invalid).await.is_err());
    assert!(Http::send("NOT A METHOD", "https://a.test").await.is_err());
    Http::assert_nothing_sent();
}

#[test]
fn response_futures_are_send() {
    fn assert_send<T: Send + 'static>(_: &T) {}

    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container);
    let future: ResponseFuture = Http::get("https://a.test");
    assert_send(&future);
    assert_eq!(future.key(), None);
    assert!(format!("{future:?}").contains("ResponseFuture"));
}
