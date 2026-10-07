//! Real requests against a local server: no fakes, no internet.

mod common;

use std::time::{Duration, Instant};

use common::{TestServer, fixture, fresh_app};
use illuminate_http_client::{ConnectionException, Http, RequestException};
use illuminate_support::{Sleep, json};

#[tokio::test]
async fn it_gets_json() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::get(server.url("/json")).await.unwrap();

    assert!(response.ok());
    assert_eq!(response["name"], "Taylor");
    assert_eq!(response.json_path("versions.2"), 13);
    assert_eq!(response.header("Content-Type"), "application/json");
    assert_eq!(response.effective_uri(), Some(server.url("/json").as_str()));
    assert_eq!(response.version(), http::Version::HTTP_11);
}

#[tokio::test]
async fn it_reads_text_and_headers() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::get(server.url("/text")).await.unwrap();
    assert_eq!(response.body(), "Hello World");
    assert_eq!(response.header("X-Custom"), "yes");

    let response = Http::get(server.url("/headers")).await.unwrap();
    assert_eq!(response.header("X-Many"), "a, b");
    assert_eq!(response.cookies().value("flavor"), Some("chocolate"));
}

#[tokio::test]
async fn it_sends_json_bodies() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::post(
        server.url("/echo"),
        json!({"name": "Steve", "role": "Network Administrator"}),
    )
    .await
    .unwrap();

    assert_eq!(response["method"], "POST");
    assert_eq!(response["headers"]["content-type"], "application/json");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(response["body"].as_str().unwrap()).unwrap(),
        json!({"name": "Steve", "role": "Network Administrator"})
    );

    for (method, response) in [
        (
            "PUT",
            Http::put(server.url("/echo"), json!({"a": 1}))
                .await
                .unwrap(),
        ),
        (
            "PATCH",
            Http::patch(server.url("/echo"), json!({"a": 1}))
                .await
                .unwrap(),
        ),
        ("DELETE", Http::delete(server.url("/echo")).await.unwrap()),
        (
            "DELETE",
            Http::delete_with(server.url("/echo"), json!({"a": 1}))
                .await
                .unwrap(),
        ),
        (
            "QUERY",
            Http::query(server.url("/echo"), json!({"a": 1}))
                .await
                .unwrap(),
        ),
        (
            "OPTIONS",
            Http::send("options", server.url("/echo")).await.unwrap(),
        ),
    ] {
        assert_eq!(response["method"], method);
    }

    let head = Http::head(server.url("/json")).await.unwrap();
    assert!(head.ok());
    assert!(head.body().is_empty());
}

#[tokio::test]
async fn it_sends_forms_and_raw_bodies() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::as_form()
        .post(
            server.url("/echo"),
            json!({"name": "Sara", "role": "Privacy Consultant"}),
        )
        .await
        .unwrap();
    assert_eq!(
        response["headers"]["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(response["body"], "name=Sara&role=Privacy+Consultant");

    let response = Http::with_body("<user>Taylor</user>", "application/xml")
        .post(server.url("/echo"), ())
        .await
        .unwrap();
    assert_eq!(response["headers"]["content-type"], "application/xml");
    assert_eq!(response["body"], "<user>Taylor</user>");
}

#[tokio::test]
async fn it_sends_multipart_requests() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::attach("attachment", b"\x89PNG fake image".to_vec(), "photo.png")
        .attach_with_headers(
            "notes",
            "Some notes",
            "notes.txt",
            &[("Content-Type", "text/markdown")],
        )
        .post(server.url("/multipart"), json!({"name": "Taylor"}))
        .await
        .unwrap();

    assert!(response.ok(), "{}", response.body());
    let fields = response.json();
    assert_eq!(fields[0]["name"], "name");
    assert_eq!(fields[0]["text"], "Taylor");
    assert!(fields[0]["filename"].is_null());
    assert_eq!(fields[1]["name"], "attachment");
    assert_eq!(fields[1]["filename"], "photo.png");
    assert_eq!(fields[1]["content_type"], "image/png");
    assert_eq!(fields[2]["filename"], "notes.txt");
    assert_eq!(fields[2]["content_type"], "text/markdown");
    assert_eq!(fields[2]["text"], "Some notes");
}

#[tokio::test]
async fn it_sends_query_parameters_and_headers() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::with_query_parameters(json!({"page": 1}))
        .with_headers([("X-First", "foo"), ("X-Second", "bar")])
        .accept_json()
        .with_user_agent("Laravel/13.0")
        .get_with(server.url("/echo?sort=name"), [("name", "Taylor Otwell")])
        .await
        .unwrap();

    assert_eq!(response["query"], "sort=name&page=1&name=Taylor%20Otwell");
    assert_eq!(response["headers"]["x-first"], "foo");
    assert_eq!(response["headers"]["x-second"], "bar");
    assert_eq!(response["headers"]["accept"], "application/json");
    assert_eq!(response["headers"]["user-agent"], "Laravel/13.0");
    assert_eq!(response["headers"]["host"], server.addr.to_string());
}

#[tokio::test]
async fn it_expands_url_templates_against_a_base_url() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::base_url(server.url(""))
        .with_url_parameters(json!({"resource": "echo", "id": 42}))
        .get("/{resource}/{id}")
        .await
        .unwrap();

    assert_eq!(response["path"], "/echo/42");
}

#[tokio::test]
async fn it_authenticates() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    assert!(
        Http::get(server.url("/basic"))
            .await
            .unwrap()
            .unauthorized()
    );

    let basic = Http::with_basic_auth("taylor", "secret")
        .get(server.url("/basic"))
        .await
        .unwrap();
    assert_eq!(basic.body(), "basic ok");

    let digest = Http::with_digest_auth("taylor", "secret")
        .get(server.url("/digest"))
        .await
        .unwrap();
    assert_eq!(digest.body(), "digest ok");

    let wrong = Http::with_digest_auth("taylor", "wrong")
        .get(server.url("/digest"))
        .await
        .unwrap();
    assert!(wrong.unauthorized());

    let token = Http::with_token("abc", "Bearer")
        .get(server.url("/echo"))
        .await
        .unwrap();
    assert_eq!(token["headers"]["authorization"], "Bearer abc");
}

#[tokio::test]
async fn it_reports_status_errors() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::get(server.url("/status/404")).await.unwrap();
    assert!(response.not_found() && response.client_error() && response.failed());

    let error = response.throw().unwrap_err();
    assert_eq!(
        error.to_string(),
        "HTTP request returned status code 404:\nstatus 404\n"
    );

    let error = Http::throw()
        .get(server.url("/status/503"))
        .await
        .unwrap_err();
    let exception = error.downcast_ref::<RequestException>().unwrap();
    assert!(exception.response.server_error());
    assert_eq!(exception.code(), 503);

    assert!(
        Http::throw_if(false)
            .get(server.url("/status/500"))
            .await
            .is_ok()
    );
    assert!(
        Http::throw_unless_fn(|r| r.status() == 500)
            .get(server.url("/status/500"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn it_follows_redirects() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::get(server.url("/redirect/3")).await.unwrap();
    assert_eq!(response["path"], "/echo");
    assert_eq!(response.effective_uri(), Some(server.url("/echo").as_str()));

    let response = Http::post(server.url("/see-other"), json!({"a": 1}))
        .await
        .unwrap();
    assert_eq!(response["method"], "GET");
    assert_eq!(response["query"], "from=see-other");
    assert_eq!(response["body"], "");

    let response = Http::post(server.url("/temporary"), json!({"a": 1}))
        .await
        .unwrap();
    assert_eq!(response["method"], "POST");
    assert_eq!(response["body"], r#"{"a":1}"#);

    let response = Http::without_redirecting()
        .get(server.url("/redirect/1"))
        .await
        .unwrap();
    assert!(response.found());
    assert!(response.redirect());
    assert_eq!(response.header("Location"), "/echo");

    let error = Http::get(server.url("/redirect-loop")).await.unwrap_err();
    assert!(error.is::<ConnectionException>());
    assert_eq!(error.to_string(), "Will not follow more than 5 redirects");

    let error = Http::max_redirects(1)
        .get(server.url("/redirect/2"))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Will not follow more than 1 redirects");
}

#[tokio::test]
async fn redirects_keep_cookies_but_not_credentials_across_origins() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let response = Http::get(server.url("/login")).await.unwrap();
    assert_eq!(response["headers"]["cookie"], "session=abc123");
    assert_eq!(response.cookies().value("session"), Some("abc123"));

    let response = Http::with_cookies([("theme", "dark")], "127.0.0.1")
        .get(server.url("/echo"))
        .await
        .unwrap();
    assert_eq!(response["headers"]["cookie"], "theme=dark");

    let response = Http::with_token("secret", "Bearer")
        .get(server.url("/elsewhere"))
        .await
        .unwrap();
    assert_eq!(response["path"], "/echo");
    assert!(response["headers"]["authorization"].is_null());
}

#[tokio::test]
async fn it_times_out() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let started = Instant::now();
    let error = Http::timeout(0.2)
        .get(server.url("/slow"))
        .await
        .unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(error.is::<ConnectionException>());
    assert_eq!(
        error.to_string(),
        "Operation timed out after 200 milliseconds with 0 bytes received"
    );
}

#[tokio::test]
async fn connection_failures_are_connection_exceptions() {
    let _app = fresh_app();

    // Bind a port, then free it, so nothing is listening there.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let error = Http::get(format!("http://127.0.0.1:{port}/"))
        .await
        .unwrap_err();
    assert!(error.is::<ConnectionException>());
    assert!(
        error
            .to_string()
            .starts_with(&format!("Failed to connect to 127.0.0.1 port {port}")),
        "{error}"
    );

    let error = Http::get("not a url").await.unwrap_err();
    assert!(error.to_string().starts_with("Invalid URL [not a url]"));
}

#[tokio::test]
async fn it_retries_failed_requests() {
    let _app = fresh_app();
    let server = TestServer::start().await;
    Sleep::fake();

    let response = Http::retry(3, 100).get(server.url("/flaky")).await.unwrap();
    assert_eq!(response.body(), "recovered on attempt 3");
    Sleep::assert_sequence(vec![
        Sleep::for_(100).milliseconds(),
        Sleep::for_(100).milliseconds(),
    ]);

    let error = Http::retry([10, 20], 0)
        .get(server.url("/status/500"))
        .await
        .unwrap_err();
    assert!(error.is::<RequestException>());
    Sleep::assert_slept_times(4);

    let response = Http::retry(2, 0)
        .retry_throw(false)
        .get(server.url("/status/500"))
        .await
        .unwrap();
    assert!(response.server_error());

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let error = Http::retry(2, 5)
        .retry_throw(false)
        .get(format!("http://127.0.0.1:{port}"))
        .await
        .unwrap_err();
    assert!(error.is::<ConnectionException>());
    Sleep::assert_slept_times(5);
    Sleep::stop_faking();
}

#[tokio::test]
async fn it_reuses_connections() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    for _ in 0..3 {
        assert!(Http::get(server.url("/json")).await.unwrap().ok());
    }
    assert_eq!(server.connections(), 1);

    // Servers that close the connection get a fresh one next time.
    Http::get(server.url("/close")).await.unwrap();
    Http::get(server.url("/json")).await.unwrap();
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn it_sends_pools_concurrently() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    let started = Instant::now();
    let responses = Http::pool(|pool| {
        vec![
            pool.timeout(5).get(server.url("/slow")),
            pool.as_("slow").get(server.url("/slow")),
            pool.get(server.url("/json")),
            pool.timeout(0.1).get(server.url("/slow")),
        ]
    })
    .await;

    assert!(
        started.elapsed() < Duration::from_millis(3500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(responses.len(), 4);
    assert_eq!(responses[0].body(), "finally");
    assert_eq!(responses["slow"].body(), "finally");
    assert_eq!(responses[1]["name"], "Taylor");
    assert!(
        responses
            .get(2)
            .unwrap()
            .as_ref()
            .unwrap_err()
            .is::<ConnectionException>()
    );
}

#[tokio::test]
async fn it_sinks_responses_to_files() {
    let _app = fresh_app();
    let server = TestServer::start().await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("download.txt");

    let response = Http::sink(&path)
        .get(server.url("/download"))
        .await
        .unwrap();

    assert_eq!(response.body(), "downloaded contents");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "downloaded contents"
    );
}

#[tokio::test]
async fn recording_captures_real_requests() {
    let _app = fresh_app();
    let server = TestServer::start().await;

    Http::record();
    Http::post(server.url("/echo"), json!({"name": "Taylor"}))
        .await
        .unwrap();

    Http::assert_sent(|request| {
        request.url() == server.url("/echo") && request["name"] == "Taylor"
    });
    let recorded = Http::recorded();
    assert_eq!(recorded[0].1.as_ref().unwrap()["method"], "POST");
}

#[tokio::test]
async fn it_speaks_tls() {
    let _app = fresh_app();
    let server = TestServer::start_tls().await;

    // The certificate isn't signed by a public CA, so verification fails...
    let error = Http::get(server.localhost_url("/json")).await.unwrap_err();
    assert!(error.is::<ConnectionException>());
    assert!(error.to_string().contains("SSL connect error"), "{error}");

    // ...unless we trust the test CA...
    let ca = fixture("ca.pem");
    let response = Http::with_options(json!({"verify": ca.to_string_lossy()}))
        .get(server.localhost_url("/json"))
        .await
        .unwrap();
    assert_eq!(response["name"], "Taylor");

    // ...including when connecting by IP address...
    let response = Http::with_options(json!({"verify": ca.to_string_lossy()}))
        .post(server.url("/echo"), json!({"secure": true}))
        .await
        .unwrap();
    assert_eq!(response["body"], r#"{"secure":true}"#);

    // ...or skip verification entirely.
    let response = Http::without_verifying()
        .get(server.localhost_url("/json"))
        .await
        .unwrap();
    assert!(response.ok());
}
