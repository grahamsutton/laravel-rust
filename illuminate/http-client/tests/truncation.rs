//! The global exception truncation setting is process-wide, so it gets a
//! test binary of its own.

use illuminate_http_client::{RequestException, Response};

#[test]
fn request_exception_truncation_can_be_configured_globally() {
    let response = Response::new(500, Default::default(), "abcdefghij");

    RequestException::truncate_at(4);
    assert_eq!(
        RequestException::new(response.clone()).message(),
        "HTTP request returned status code 500:\nabcd (truncated...)\n"
    );

    // A per-response setting wins over the global one.
    let mut custom = response.clone();
    custom.truncate_exceptions_at(2);
    assert_eq!(
        custom.throw().unwrap_err().message(),
        "HTTP request returned status code 500:\nab (truncated...)\n"
    );

    RequestException::dont_truncate();
    assert!(
        RequestException::new(response.clone())
            .message()
            .contains("HTTP/1.1 500 Internal Server Error")
    );

    RequestException::truncate();
    assert_eq!(
        RequestException::new(response).message(),
        "HTTP request returned status code 500:\nabcdefghij\n"
    );
}
