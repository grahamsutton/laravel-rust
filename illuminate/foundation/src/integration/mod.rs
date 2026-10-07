//! The glue between framework components: hooks that let independent
//! crates cooperate without depending on each other.

use std::sync::{Arc, LazyLock, RwLock};

use illuminate_container::try_app;
use illuminate_http::{ExceptionHandler, current_request};
use illuminate_log::{Level, Log};
use illuminate_support::{Error, Map, Value, ValueExt};

use crate::exceptions::{Handler, Prepared};

mod auth;
mod views;

type ViewRenderer = Arc<dyn Fn(&str, Value) -> Option<String> + Send + Sync>;

static VIEW_RENDERER: LazyLock<RwLock<Option<ViewRenderer>>> = LazyLock::new(|| RwLock::new(None));

/// Set the function used to render views outside of a request (error pages,
/// maintenance templates).
pub fn set_view_renderer(renderer: impl Fn(&str, Value) -> Option<String> + Send + Sync + 'static) {
    *VIEW_RENDERER.write().unwrap() = Some(Arc::new(renderer));
}

/// The view renderer, if the view layer has been wired up.
pub fn view_renderer() -> Option<ViewRenderer> {
    VIEW_RENDERER.read().unwrap().clone()
}

/// Wire every component together. Runs when the foundation provider boots.
pub fn boot() {
    wire_exception_handler();
    wire_pagination();
    wire_validation();
    wire_filesystem();
    views::boot();
    auth::boot();
    wire_events();
    wire_eloquent();
    wire_transactions();
    wire_context();
}

/// The `Context` travels with the jobs a request dispatches, and is
/// restored while the worker runs them.
fn wire_context() {
    use illuminate_log::Context;
    use illuminate_queue::Queue;
    use illuminate_queue::events::{JobExceptionOccurred, JobProcessed, JobProcessing};

    const KEY: &str = "illuminate:log:context";

    Queue::create_payload_using(|_, _, _| {
        let mut payload = Map::new();
        if let Some(context) = Context::dehydrate() {
            payload.insert(KEY.to_string(), context);
        }
        payload
    });

    // A worker outside a request starts each job with a clean context.
    let forget = || {
        if current_request().is_none() {
            Context::flush();
        }
    };
    Queue::listen::<JobProcessing>(move |event| {
        forget();
        if let Some(context) = event.job.payload().get(KEY) {
            Context::hydrate(context);
        }
    });
    Queue::listen::<JobProcessed>(move |_| forget());
    Queue::listen::<JobExceptionOccurred>(move |_| forget());
}

/// Jobs dispatched `after_commit` wait for the open database transaction.
fn wire_transactions() {
    illuminate_container::Container::get_instance()
        .instance_arc::<dyn illuminate_queue::TransactionManager>(Arc::new(DatabaseTransactions));
}

/// The queue's view of the database's transactions.
struct DatabaseTransactions;

impl DatabaseTransactions {
    /// The resolved connection with an open transaction, if any.
    fn open() -> Option<illuminate_database::Connection> {
        let manager = try_app::<illuminate_database::DatabaseManager>()?;
        manager
            .get_connections()
            .into_iter()
            .map(|name| manager.connection(&name))
            .find(|connection| connection.transaction_level() > 0)
    }
}

impl illuminate_queue::TransactionManager for DatabaseTransactions {
    fn in_transaction(&self) -> bool {
        Self::open().is_some()
    }

    fn add_callback(&self, callback: illuminate_queue::TransactionCallback) {
        let callback: illuminate_database::TransactionCallback = callback;
        let unclaimed = match Self::open() {
            Some(connection) => connection.defer_until_commit(callback).err(),
            None => Some(callback),
        };
        // The transaction closed in the meantime: nothing to wait for.
        if let Some(callback) = unclaimed {
            tokio::spawn(illuminate_container::Container::scope_current(callback()));
        }
    }

    fn add_callback_for_rollback(&self, callback: illuminate_queue::TransactionCallback) {
        if let Some(connection) = Self::open() {
            let _ = connection.defer_until_rollback(callback);
        }
    }
}

/// Eloquent hashes `#[hashed]` attributes with the application's hasher,
/// and a missing route-bound model is a `404`.
fn wire_eloquent() {
    use illuminate_database::eloquent::{self, ModelNotFoundException};

    eloquent::hash_using(|value| {
        illuminate_hashing::Hash::make(value).expect("Unable to hash the attribute value")
    });

    illuminate_routing::Route::set_missing_model_detector(|error| error.is::<ModelNotFoundException>());

    // Model events reach event listeners as `eloquent.created: User`.
    struct EloquentEvents;

    #[async_trait::async_trait]
    impl eloquent::EventDispatcher for EloquentEvents {
        fn has_listeners(&self, event: &str) -> bool {
            let events = illuminate_events::Event::dispatcher();
            events.is_fake() || events.has_listeners_named(event)
        }

        async fn until(&self, event: &str, payload: Value) -> illuminate_support::Result<bool> {
            illuminate_events::Event::until_named(event, payload).await
        }
    }

    eloquent::set_event_dispatcher(EloquentEvents);
    eloquent::report_exceptions_using(crate::helpers::report);

    if let Some(handler) = try_app::<Handler>() {
        handler.configure(|exceptions| {
            exceptions.prepare_using(|error| {
                error.downcast_ref::<ModelNotFoundException>().map(|e| Prepared::Http {
                    status: 404,
                    message: e.to_string(),
                    headers: Vec::new(),
                })
            });
        });
    }
}

/// Queued event listeners (`ShouldQueue`) are pushed onto the queue.
fn wire_events() {
    use illuminate_queue::{CallQueuedClosure, Dispatchable};

    illuminate_events::Event::queue_listeners_using(|listener: illuminate_events::QueuedListener| async move {
        let name = illuminate_support::Str::class_basename(listener.listener);
        let (connection, queue, delay) = (listener.connection.clone(), listener.queue.clone(), listener.delay);
        let mut pending = CallQueuedClosure::once(move || listener.handle()).name(name).dispatch();
        if let Some(connection) = connection {
            pending = pending.on_connection(connection);
        }
        if let Some(queue) = queue {
            pending = pending.on_queue(queue);
        }
        if let Some(delay) = delay {
            pending = pending.delay(delay);
        }
        pending.await
    });
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
                .downcast_ref::<illuminate_validation::ValidationException>()
                .map(|e| Prepared::Validation {
                    message: e.to_string(),
                    errors: e.errors.clone(),
                    status: e.status,
                    error_bag: e.error_bag.clone(),
                    redirect_to: e.redirect_to.clone(),
                })
        });
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

/// Validation messages come from the application's `lang` directory first,
/// falling back to Laravel's English lines.
fn wire_validation() {
    struct TranslatedMessages;

    impl illuminate_validation::MessageResolver for TranslatedMessages {
        fn get(&self, key: &str) -> Option<Value> {
            if illuminate_translation::Lang::has(key) {
                let line = illuminate_translation::Lang::get_value(key, &Value::Object(Map::new()), None, true);
                if !line.is_null() {
                    return Some(line);
                }
            }
            illuminate_validation::EnglishMessages.get(key)
        }
    }

    illuminate_validation::Validator::resolve_messages_using(TranslatedMessages);
}

/// Local disks configured with `serve => true` hand out signed URLs, and get
/// a `storage.{disk}` route serving their files.
fn wire_filesystem() {
    use illuminate_filesystem::{ServeFile, Storage, UrlSigner};
    use illuminate_http::Request;
    use illuminate_routing::{Path, Route, URL};
    use illuminate_support::{Carbon, Result};

    struct RoutingUrlSigner;

    impl UrlSigner for RoutingUrlSigner {
        fn temporary_signed_route(&self, name: &str, expiration: Carbon, parameters: Value) -> Result<String> {
            // Signed relative to the app, so it survives proxies and host changes.
            let relative = URL::signed_route_with(name, parameters, Some(expiration), false)?;
            Ok(URL::to(&relative))
        }

        fn has_valid_relative_signature(&self, request: &Request) -> bool {
            URL::has_valid_relative_signature(request)
        }
    }

    let Some(app) = crate::Application::try_current() else {
        return;
    };
    app.container().instance_arc::<dyn UrlSigner>(Arc::new(RoutingUrlSigner));

    let Ok(disks) = Storage::manager().and_then(|manager| manager.served_disks()) else {
        return;
    };
    let is_production = app.is_production();
    for disk in disks {
        let serve = Arc::new(ServeFile::new(disk.disk.clone(), disk.config.clone(), is_production));
        Route::get(&disk.route_uri(), move |request: Request, Path(path): Path<String>| {
            let serve = serve.clone();
            async move { serve.handle(&request, &path).await }
        })
        .where_("path", ".*")
        .name(&disk.route_name());
    }
}
