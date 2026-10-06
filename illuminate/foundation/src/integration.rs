//! The glue between framework components: hooks that let independent
//! crates cooperate without depending on each other.

use std::sync::Arc;

use illuminate_container::try_app;
use illuminate_http::{ExceptionHandler, current_request};
use illuminate_log::{Level, Log};
use illuminate_support::{Error, Map, Value, ValueExt};

use crate::exceptions::{Handler, Prepared};

/// Wire every component together. Runs when the foundation provider boots.
pub fn boot() {
    wire_exception_handler();
    wire_pagination();
}

/// Report exceptions through the log, and teach the handler about the
/// exceptions other components throw.
fn wire_exception_handler() {
    let Some(handler) = try_app::<Handler>() else {
        return;
    };

    handler.report_using(|error: &Error, context: Value| {
        let level = context
            .get("level")
            .and_then(Value::as_str)
            .and_then(Level::parse)
            .unwrap_or(Level::Error);
        let mut context = context;
        if let Value::Object(map) = &mut context {
            map.shift_remove("level");
        }
        Log::log_with(level, error, context);
    });

    handler.configure(|exceptions| {
        exceptions.prepare_using(|error| {
            error
                .downcast_ref::<illuminate_session::TokenMismatchException>()
                .map(|_| Prepared::Http {
                    status: 419,
                    message: "Page Expired".to_string(),
                    headers: Vec::new(),
                })
        });
        exceptions.prepare_using(|error| {
            error
                .downcast_ref::<illuminate_routing::InvalidSignatureException>()
                .map(|e| Prepared::Http {
                    status: 403,
                    message: e.to_string(),
                    headers: Vec::new(),
                })
        });
        exceptions.prepare_using(|error| {
            error
                .downcast_ref::<illuminate_filesystem::exceptions::FileNotFoundException>()
                .map(|_| Prepared::Http {
                    status: 404,
                    message: "Not Found".to_string(),
                    headers: Vec::new(),
                })
        });
    });

    // Let `ExceptionHandler` resolve to the configured handler.
    let _: Option<Arc<dyn ExceptionHandler>> = try_app::<dyn ExceptionHandler>();
}

/// Paginators read the page, path, and query string from the current request.
fn wire_pagination() {
    illuminate_pagination::resolve_current_page_using(|page_name| {
        let request = current_request()?;
        request
            .input(page_name)
            .to_i64_lossy()
            .filter(|page| *page >= 1)
            .map(|page| page as u64)
    });
    illuminate_pagination::resolve_current_path_using(|| {
        current_request()
            .map(|request| request.url())
            .unwrap_or_else(|| "/".to_string())
    });
    illuminate_pagination::resolve_query_string_using(|| match current_request().map(|r| r.query_all()) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    });
    illuminate_pagination::translate_using(|key| {
        let line = illuminate_translation::Lang::get(key);
        (line != key).then_some(line)
    });
}
