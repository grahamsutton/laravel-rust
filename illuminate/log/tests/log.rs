use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_log::{
    Level, Log, LogManager, LogServiceProvider, MessageLogged, Monolog, StackChannel, TestHandler,
    info, logger,
};
use illuminate_support::{Carbon, Value, json};

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

/// `Carbon::set_test_now` is global, so tests that freeze time take turns.
static CLOCK: Mutex<()> = Mutex::new(());

struct Frozen {
    _guard: MutexGuard<'static, ()>,
}

impl Drop for Frozen {
    fn drop(&mut self) {
        Carbon::set_test_now(None);
    }
}

fn freeze(at: &str) -> Frozen {
    let guard = CLOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    travel(at);
    Frozen { _guard: guard }
}

fn travel(at: &str) {
    Carbon::set_test_now(Some(Carbon::parse(at).unwrap()));
}

fn manager(config: Value) -> LogManager {
    LogManager::new(Arc::new(Repository::new(config)))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn single(path: &Path) -> Value {
    json!({"driver": "single", "path": path})
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ----------------------------------------------------------------------
// Line format
// ----------------------------------------------------------------------

#[test]
fn single_channel_writes_laravel_formatted_lines() {
    let _now = freeze("2024-01-01 12:00:00");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logs/laravel.log");
    let log = manager(json!({
        "app": {"env": "local"},
        "logging": {"default": "single", "channels": {"single": single(&path)}},
    }));

    log.info("User logged in.");
    log.info_with("User logged in.", json!({"id": 1}));
    log.error_with("Failed.", json!({"user": {"id": 1, "roles": ["admin"]}, "url": "https://laravel.com/docs"}));

    assert_eq!(
        read(&path),
        concat!(
            "[2024-01-01 12:00:00] local.INFO: User logged in.  \n",
            "[2024-01-01 12:00:00] local.INFO: User logged in. {\"id\":1} \n",
            "[2024-01-01 12:00:00] local.ERROR: Failed. {\"user\":{\"id\":1,\"roles\":[\"admin\"]},\"url\":\"https://laravel.com/docs\"} \n",
        )
    );
}

#[test]
fn every_level_has_its_name() {
    let _now = freeze("2024-05-06 07:08:09");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("laravel.log");
    let log = manager(json!({
        "app": {"env": "production"},
        "logging": {"default": "single", "channels": {"single": single(&path)}},
    }));

    log.emergency("a");
    log.alert("b");
    log.critical("c");
    log.error("d");
    log.warning("e");
    log.notice("f");
    log.info("g");
    log.debug("h");
    log.log(Level::Notice, "i");
    log.log_with(Level::Debug, "j", json!({"k": true}));

    let lines: Vec<String> = read(&path).lines().map(String::from).collect();
    assert_eq!(
        lines,
        vec![
            "[2024-05-06 07:08:09] production.EMERGENCY: a  ",
            "[2024-05-06 07:08:09] production.ALERT: b  ",
            "[2024-05-06 07:08:09] production.CRITICAL: c  ",
            "[2024-05-06 07:08:09] production.ERROR: d  ",
            "[2024-05-06 07:08:09] production.WARNING: e  ",
            "[2024-05-06 07:08:09] production.NOTICE: f  ",
            "[2024-05-06 07:08:09] production.INFO: g  ",
            "[2024-05-06 07:08:09] production.DEBUG: h  ",
            "[2024-05-06 07:08:09] production.NOTICE: i  ",
            "[2024-05-06 07:08:09] production.DEBUG: j {\"k\":true} ",
        ]
    );
}

#[test]
fn the_channel_name_falls_back_to_production_and_may_be_configured() {
    let _now = freeze("2024-01-01 00:00:00");
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.log");
    let b = dir.path().join("b.log");
    let log = manager(json!({
        "logging": {"channels": {
            "a": single(&a),
            "b": {"driver": "single", "path": b, "name": "billing"},
        }},
    }));

    log.channel("a").info("hi");
    log.channel("b").info("hi");

    assert_eq!(read(&a), "[2024-01-01 00:00:00] production.INFO: hi  \n");
    assert_eq!(read(&b), "[2024-01-01 00:00:00] billing.INFO: hi  \n");
}

#[test]
fn messages_keep_their_line_breaks() {
    let _now = freeze("2024-01-01 00:00:00");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("laravel.log");
    let log = manager(json!({"app": {"env": "local"}, "logging": {"channels": {"single": single(&path)}}}));

    log.channel("single")
        .error_with("Something broke\non two lines", json!({"exception": "[object] (Error(code: 0): Boom)\n[stacktrace]\n#0 main"}));

    assert_eq!(
        read(&path),
        "[2024-01-01 00:00:00] local.ERROR: Something broke\non two lines {\"exception\":\"[object] (Error(code: 0): Boom)\n[stacktrace]\n#0 main\"} \n"
    );
}

#[test]
fn placeholders_are_replaced_when_enabled() {
    let _now = freeze("2024-01-01 00:00:00");
    let dir = tempfile::tempdir().unwrap();
    let on = dir.path().join("on.log");
    let off = dir.path().join("off.log");
    let log = manager(json!({"app": {"env": "local"}, "logging": {"channels": {
        "on": {"driver": "single", "path": on, "replace_placeholders": true},
        "off": single(&off),
    }}}));

    log.channel("on").info_with("Showing the user profile for user: {id}", json!({"id": 7}));
    log.channel("off").info_with("Showing the user profile for user: {id}", json!({"id": 7}));

    assert_eq!(
        read(&on),
        "[2024-01-01 00:00:00] local.INFO: Showing the user profile for user: 7 {\"id\":7} \n"
    );
    assert_eq!(
        read(&off),
        "[2024-01-01 00:00:00] local.INFO: Showing the user profile for user: {id} {\"id\":7} \n"
    );
}

#[test]
fn json_formatter_can_be_configured() {
    let _now = freeze("2024-01-01 12:00:00");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("json.log");
    let log = manager(json!({"app": {"env": "local"}, "logging": {"channels": {
        "json": {"driver": "single", "path": path, "formatter": "json"},
    }}}));

    log.channel("json").info_with("Hello", json!({"id": 1}));

    assert_eq!(
        read(&path),
        "{\"message\":\"Hello\",\"context\":{\"id\":1},\"level\":200,\"level_name\":\"INFO\",\"channel\":\"local\",\"datetime\":\"2024-01-01T12:00:00.000000+00:00\",\"extra\":{}}\n"
    );
}

#[test]
fn line_formatter_options_can_be_configured() {
    let _now = freeze("2024-01-01 12:00:00");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("custom.log");
    let log = manager(json!({"app": {"env": "local"}, "logging": {"channels": {
        "custom": {
            "driver": "single",
            "path": path,
            "formatter": "Monolog\\Formatter\\LineFormatter",
            "formatter_with": {"format": "%level_name%|%message%|%context%\n", "dateFormat": "Y"},
        },
    }}}));

    log.channel("custom").warning("Careful");

    assert_eq!(read(&path), "WARNING|Careful|[]\n");
}

// ----------------------------------------------------------------------
// Levels
// ----------------------------------------------------------------------

#[test]
fn channels_ignore_messages_below_their_level() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("laravel.log");
    let log = manager(json!({"logging": {"default": "single", "channels": {
        "single": {"driver": "single", "path": path, "level": "warning"},
    }}}));

    log.debug("debug");
    log.info("info");
    log.notice("notice");
    log.warning("warning");
    log.error("error");
    log.emergency("emergency");

    let contents = read(&path);
    assert!(!contents.contains("debug") && !contents.contains(".INFO") && !contents.contains("notice"));
    assert_eq!(contents.lines().count(), 3);
    assert!(contents.contains("production.WARNING: warning"));
    assert!(contents.contains("production.EMERGENCY: emergency"));
}

#[test]
fn invalid_levels_fall_back_to_the_emergency_logger() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("laravel.log");
    let emergency = dir.path().join("emergency.log");
    let log = manager(json!({"logging": {"channels": {
        "single": {"driver": "single", "path": path, "level": "loud"},
        "emergency": {"path": emergency},
    }}}));

    log.channel("single").info("Hello");

    let contents = read(&emergency);
    assert!(contents.contains(
        "laravel.EMERGENCY: Unable to create configured logger. Using emergency logger. {\"exception\":\"[object] (InvalidArgumentException(code: 0): Invalid log level.)\"}"
    ));
    assert!(contents.contains("laravel.INFO: Hello"));
    assert!(!path.exists());
}

#[test]
fn undefined_channels_use_the_emergency_logger() {
    let dir = tempfile::tempdir().unwrap();
    let emergency = dir.path().join("emergency.log");
    let log = manager(json!({"logging": {"channels": {"emergency": {"path": emergency}}}}));

    log.channel("missing").warning("Where am I?");

    let contents = read(&emergency);
    assert!(contents.contains("(InvalidArgumentException(code: 0): Log [missing] is not defined.)"));
    assert!(contents.contains("laravel.WARNING: Where am I?"));
}

#[test]
fn unsupported_drivers_use_the_emergency_logger() {
    let dir = tempfile::tempdir().unwrap();
    let emergency = dir.path().join("emergency.log");
    let log = manager(json!({"logging": {"channels": {
        "slack": {"driver": "slack"},
        "emergency": {"path": emergency},
    }}}));

    log.channel("slack").critical("Down!");

    assert!(read(&emergency).contains("Driver [slack] is not supported."));
}

// ----------------------------------------------------------------------
// Daily & monthly
// ----------------------------------------------------------------------

#[test]
fn daily_channels_write_one_file_per_day() {
    let _now = freeze("2024-01-01 23:59:59");
    let dir = tempfile::tempdir().unwrap();
    let log = manager(json!({"app": {"env": "local"}, "logging": {"default": "daily", "channels": {
        "daily": {"driver": "daily", "path": dir.path().join("laravel.log"), "days": 14},
    }}}));

    log.info("first");
    travel("2024-01-02 00:00:01");
    log.info("second");

    assert_eq!(files_in(dir.path()), vec!["laravel-2024-01-01.log", "laravel-2024-01-02.log"]);
    assert_eq!(
        read(&dir.path().join("laravel-2024-01-01.log")),
        "[2024-01-01 23:59:59] local.INFO: first  \n"
    );
    assert_eq!(
        read(&dir.path().join("laravel-2024-01-02.log")),
        "[2024-01-02 00:00:01] local.INFO: second  \n"
    );
}

#[test]
fn daily_channels_delete_files_beyond_the_retention() {
    let _now = freeze("2024-03-01 10:00:00");
    let dir = tempfile::tempdir().unwrap();
    // Files from previous runs, plus an unrelated file that must survive.
    for name in ["laravel-2024-02-26.log", "laravel-2024-02-27.log", "laravel-2024-02-28.log", "other.log"] {
        fs::write(dir.path().join(name), "old\n").unwrap();
    }
    let log = manager(json!({"logging": {"default": "daily", "channels": {
        "daily": {"driver": "daily", "path": dir.path().join("laravel.log"), "days": 2},
    }}}));

    log.info("today");
    assert_eq!(
        files_in(dir.path()),
        vec!["laravel-2024-02-28.log", "laravel-2024-03-01.log", "other.log"]
    );

    travel("2024-03-02 10:00:00");
    log.info("tomorrow");
    log.info("tomorrow again");
    assert_eq!(
        files_in(dir.path()),
        vec!["laravel-2024-03-01.log", "laravel-2024-03-02.log", "other.log"]
    );
    assert_eq!(read(&dir.path().join("laravel-2024-03-02.log")).lines().count(), 2);
}

#[test]
fn max_files_takes_precedence_and_zero_keeps_everything() {
    let _now = freeze("2024-03-10 10:00:00");
    let dir = tempfile::tempdir().unwrap();
    for day in 1..=5 {
        fs::write(dir.path().join(format!("app-2024-03-0{day}.log")), "old\n").unwrap();
    }
    let log = manager(json!({"logging": {"channels": {
        "all": {"driver": "daily", "path": dir.path().join("app.log"), "max_files": 0, "days": 1},
    }}}));
    log.channel("all").info("kept");
    assert_eq!(files_in(dir.path()).len(), 6);
}

#[test]
fn monthly_channels_write_one_file_per_month() {
    let _now = freeze("2024-01-31 12:00:00");
    let dir = tempfile::tempdir().unwrap();
    let log = manager(json!({"logging": {"channels": {
        "monthly": {"driver": "monthly", "path": dir.path().join("laravel.log")},
    }}}));

    log.channel("monthly").info("January");
    travel("2024-02-01 12:00:00");
    log.channel("monthly").info("February");

    assert_eq!(files_in(dir.path()), vec!["laravel-2024-01.log", "laravel-2024-02.log"]);
}

// ----------------------------------------------------------------------
// Stacks
// ----------------------------------------------------------------------

#[test]
fn stacks_fan_out_to_their_channels_respecting_levels() {
    let _now = freeze("2024-01-01 00:00:00");
    let dir = tempfile::tempdir().unwrap();
    let all = dir.path().join("all.log");
    let errors = dir.path().join("errors.log");
    let log = manager(json!({"app": {"env": "local"}, "logging": {"default": "stack", "channels": {
        "stack": {"driver": "stack", "channels": ["all", "errors"], "ignore_exceptions": false},
        "all": {"driver": "single", "path": all, "name": "ignored"},
        "errors": {"driver": "single", "path": errors, "level": "error"},
    }}}));

    log.info("Just so you know");
    log.critical("The system is down!");

    // The stack's own channel name is used for every handler.
    assert_eq!(
        read(&all),
        "[2024-01-01 00:00:00] local.INFO: Just so you know  \n[2024-01-01 00:00:00] local.CRITICAL: The system is down!  \n"
    );
    assert_eq!(read(&errors), "[2024-01-01 00:00:00] local.CRITICAL: The system is down!  \n");
}

#[test]
fn stack_channels_may_be_a_comma_separated_string_and_named() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.log");
    let b = dir.path().join("b.log");
    let log = manager(json!({"logging": {"default": "stack", "channels": {
        "stack": {"driver": "stack", "channels": "a, b", "name": "stacked"},
        "a": single(&a),
        "b": single(&b),
    }}}));

    log.info("hello");

    assert!(read(&a).contains("stacked.INFO: hello"));
    assert!(read(&b).contains("stacked.INFO: hello"));
}

#[test]
fn stacks_merge_processors_from_their_channels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.log");
    let log = manager(json!({"logging": {"default": "stack", "channels": {
        "stack": {"driver": "stack", "channels": ["a"]},
        "a": {"driver": "single", "path": path, "replace_placeholders": true},
    }}}));

    log.info_with("User {id}", json!({"id": 3}));

    assert!(read(&path).contains("INFO: User 3 {\"id\":3}"));
}

#[test]
fn stacks_may_ignore_failing_channels() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("not-a-directory");
    fs::write(&blocker, "").unwrap();
    let good = dir.path().join("good.log");
    let log = manager(json!({"logging": {"default": "stack", "channels": {
        "stack": {"driver": "stack", "channels": ["broken", "good"], "ignore_exceptions": true},
        "broken": single(&blocker.join("laravel.log")),
        "good": single(&good),
    }}}));

    log.info("still logged");

    assert!(read(&good).contains("INFO: still logged"));
}

#[test]
fn on_demand_stacks_and_channels() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.log");
    let custom = dir.path().join("custom.log");
    let log = manager(json!({"app": {"env": "testing"}, "logging": {"channels": {"a": single(&a)}}}));

    let channel = log.build(json!({"driver": "single", "path": custom}));
    log.stack([StackChannel::from("a"), channel.into()]).info("Something happened!");
    log.stack(["a"]).info("Just the one");

    assert_eq!(read(&a).lines().count(), 2);
    assert_eq!(read(&custom).lines().count(), 1);
    assert!(read(&custom).contains("testing.INFO: Something happened!"));
}

#[test]
fn on_demand_channels_are_rebuilt_each_time() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.log");
    let second = dir.path().join("second.log");
    let log = manager(json!({}));

    log.build(single(&first)).info("one");
    log.build(single(&second)).info("two");

    assert!(read(&first).contains("one") && !read(&first).contains("two"));
    assert!(read(&second).contains("two"));
}

// ----------------------------------------------------------------------
// Context
// ----------------------------------------------------------------------

#[test]
fn channel_context_is_added_to_every_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("laravel.log");
    let other = dir.path().join("other.log");
    let log = manager(json!({"logging": {"default": "single", "channels": {
        "single": single(&path),
        "other": single(&other),
    }}}));

    log.with_context(json!({"request-id": "abc"}));
    log.info_with("first", json!({"user": 1}));
    log.info_with("second", json!({"request-id": "override"}));
    log.channel("other").info("elsewhere");

    let lines: Vec<String> = read(&path).lines().map(String::from).collect();
    assert!(lines[0].ends_with("first {\"request-id\":\"abc\",\"user\":1} "));
    assert!(lines[1].ends_with("second {\"request-id\":\"override\"} "));
    assert!(read(&other).ends_with("elsewhere  \n"));

    log.without_context();
    log.info("third");
    assert!(read(&path).ends_with("third  \n"));
}

#[test]
fn shared_context_reaches_existing_and_future_channels() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.log");
    let b = dir.path().join("b.log");
    let log = manager(json!({"logging": {"channels": {"a": single(&a), "b": single(&b)}}}));

    let existing = log.channel("a");
    log.share_context(json!({"invocation-id": "xyz"}));
    existing.info("from a");
    log.channel("b").info("from b");
    log.stack(["b"]).info("from a stack");

    assert!(read(&a).contains("from a {\"invocation-id\":\"xyz\"}"));
    assert!(read(&b).contains("from b {\"invocation-id\":\"xyz\"}"));
    assert!(read(&b).contains("from a stack {\"invocation-id\":\"xyz\"}"));
    assert_eq!(log.shared_context()["invocation-id"], "xyz");

    log.flush_shared_context();
    assert!(log.shared_context().is_empty());
}

#[test]
fn context_keys_can_be_removed() {
    let handler = Arc::new(TestHandler::new());
    let log = manager(json!({"logging": {"channels": {"test": {"driver": "test"}}}}));
    let shared = handler.clone();
    log.extend("test", move |_, _| Ok(Monolog::new("testing").with_handler_arc(shared.clone())));

    let channel = log.channel("test");
    channel.with_context(json!({"a": 1, "b": 2, "c": 3}));
    log.without_context_keys(&["a", "c"]);
    channel.info("hi");

    assert_eq!(handler.records()[0].context, illuminate_log::to_context(json!({"b": 2})));
}

// ----------------------------------------------------------------------
// Listeners
// ----------------------------------------------------------------------

#[test]
fn listeners_receive_logged_messages() {
    let log = manager(json!({"logging": {"default": "null"}}));
    let messages: Arc<Mutex<Vec<MessageLogged>>> = Arc::default();
    let seen = messages.clone();
    log.listen(move |event: &MessageLogged| seen.lock().unwrap().push(event.clone()));

    log.with_context(json!({"request": 1}));
    log.warning_with("Disk space low", json!({"free": "1GB"}));

    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].level, Level::Warning);
    assert_eq!(messages[0].message, "Disk space low");
    assert_eq!(messages[0].context, illuminate_log::to_context(json!({"request": 1, "free": "1GB"})));
}

#[test]
fn listeners_only_hear_handled_levels() {
    let dir = tempfile::tempdir().unwrap();
    let log = manager(json!({"logging": {"default": "single", "channels": {
        "single": {"driver": "single", "path": dir.path().join("laravel.log"), "level": "error"},
    }}}));
    let count = Arc::new(Mutex::new(0));
    let c = count.clone();
    log.listen(move |_| *c.lock().unwrap() += 1);

    log.info("ignored");
    log.error("heard");

    assert_eq!(*count.lock().unwrap(), 1);
}

// ----------------------------------------------------------------------
// Drivers
// ----------------------------------------------------------------------

#[test]
fn custom_drivers_can_be_registered() {
    let handler = Arc::new(TestHandler::new());
    let log = manager(json!({"logging": {"default": "mongo", "channels": {
        "mongo": {"driver": "mongodb", "collection": "logs"},
        "factory": {"driver": "custom", "via": "factory"},
    }}}));

    let h = handler.clone();
    log.extend("mongodb", move |_app, config| {
        assert_eq!(config["collection"], "logs");
        Ok(Monolog::new("mongo").with_handler_arc(h.clone()))
    });
    let h = handler.clone();
    log.extend("factory", move |_, _| Ok(Monolog::new("factory").with_handler_arc(h.clone())));

    log.info("to mongo");
    log.channel("factory").info("from the factory");

    assert_eq!(handler.messages(), vec!["to mongo", "from the factory"]);
    assert_eq!(handler.records()[1].channel, "factory");
}

#[test]
fn custom_via_without_a_creator_falls_back_to_emergency() {
    let dir = tempfile::tempdir().unwrap();
    let emergency = dir.path().join("emergency.log");
    let log = manager(json!({"logging": {"channels": {
        "custom": {"driver": "custom", "via": "App\\Logging\\CreateCustomLogger"},
        "emergency": {"path": emergency},
    }}}));

    log.channel("custom").info("hi");

    assert!(read(&emergency).contains("Custom log driver [App\\\\Logging\\\\CreateCustomLogger] has not been registered."));
}

#[test]
fn monolog_driver_supports_the_built_in_handlers() {
    let _now = freeze("2024-01-01 00:00:00");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stream.log");
    let log = manager(json!({"app": {"env": "local"}, "logging": {"channels": {
        "stream": {
            "driver": "monolog",
            "level": "info",
            "handler": "Monolog\\Handler\\StreamHandler",
            "handler_with": {"stream": path},
            "processors": ["Monolog\\Processor\\PsrLogMessageProcessor"],
        },
        "null": {"driver": "monolog", "handler": "Monolog\\Handler\\NullHandler"},
        "bogus": {"driver": "monolog", "handler": "App\\Handler"},
    }}}));

    log.channel("stream").debug("skipped");
    log.channel("stream").info_with("Hi {name}", json!({"name": "Taylor"}));
    log.channel("null").info("nothing");

    assert_eq!(read(&path), "[2024-01-01 00:00:00] local.INFO: Hi Taylor {\"name\":\"Taylor\"} \n");
    assert!(log.channel("null").is_handling(Level::Debug));
    assert_eq!(log.channel("bogus").name(), "laravel", "falls back to the emergency logger");
}

#[test]
fn stream_drivers_resolve_standard_streams() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stream.log");
    let log = manager(json!({"app": {"env": "local"}, "logging": {"channels": {
        "stderr": {"driver": "stderr", "level": "emergency"},
        "stdout": {"driver": "stdout", "level": "emergency"},
        "errorlog": {"driver": "errorlog", "level": "emergency"},
        "file": {"driver": "stream", "with": {"stream": path}},
    }}}));

    for name in ["stderr", "stdout", "errorlog"] {
        let channel = log.channel(name);
        assert_eq!(channel.name(), "local");
        assert!(!channel.is_handling(Level::Alert));
        assert!(channel.is_handling(Level::Emergency));
    }
    log.channel("file").info("streamed");
    assert!(read(&path).contains("local.INFO: streamed"));
}

#[test]
fn the_null_channel_always_exists_and_is_the_default_fallback() {
    let log = manager(json!({}));
    assert_eq!(log.get_default_driver(), None);
    log.info("into the void");
    assert!(log.get_channels().contains_key("null"));
    assert!(log.driver(None).is_handling(Level::Debug));
}

#[test]
fn channels_are_cached_until_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let log = manager(json!({"logging": {"default": "single", "channels": {
        "single": single(&dir.path().join("a.log")),
    }}}));

    let first = log.channel("single");
    assert!(Arc::ptr_eq(&first, &log.channel("single")));
    assert!(Arc::ptr_eq(&first, &log.driver(None)));

    log.forget_channel(Some("single"));
    assert!(!Arc::ptr_eq(&first, &log.channel("single")));

    log.set_default_driver("other");
    assert_eq!(log.get_default_driver().as_deref(), Some("other"));
}

#[cfg(unix)]
#[test]
fn file_permissions_can_be_configured() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private.log");
    let octal = dir.path().join("octal.log");
    let log = manager(json!({"logging": {"channels": {
        "private": {"driver": "single", "path": path, "permission": 0o600},
        "octal": {"driver": "single", "path": octal, "permission": "0640"},
    }}}));

    log.channel("private").info("secret");
    log.channel("octal").info("secret");

    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&octal).unwrap().permissions().mode() & 0o777, 0o640);
}

#[test]
fn concurrent_writes_never_interleave() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("busy.log");
    let log = Arc::new(manager(json!({"logging": {"default": "single", "channels": {
        "single": single(&path),
        "same-file": single(&path),
    }}})));

    let threads: Vec<_> = (0..8)
        .map(|t| {
            let log = log.clone();
            std::thread::spawn(move || {
                for i in 0..50 {
                    let channel = if i % 2 == 0 { "single" } else { "same-file" };
                    log.channel(channel)
                        .info_with(format!("thread {t} message {i}"), json!({"padding": "x".repeat(200)}));
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }

    let contents = read(&path);
    assert_eq!(contents.lines().count(), 400);
    assert!(contents.lines().all(|line| line.starts_with('[') && line.ends_with("\"} ")));
}

#[test]
fn syslog_udp_handler_sends_rfc5424_datagrams() {
    let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    server
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let port = server.local_addr().unwrap().port();
    let log = manager(json!({"app": {"env": "local", "name": "My App"}, "logging": {"channels": {
        "papertrail": {
            "driver": "monolog",
            "handler": "Monolog\\Handler\\SyslogUdpHandler",
            "handler_with": {"host": "127.0.0.1", "port": port, "ident": "my-app"},
        },
    }}}));

    log.channel("papertrail").info("Hello, Papertrail");

    let mut buffer = [0u8; 2048];
    let (length, _) = server.recv_from(&mut buffer).unwrap();
    let datagram = String::from_utf8_lossy(&buffer[..length]).into_owned();
    assert!(datagram.starts_with("<14>1 "), "{datagram}");
    assert!(datagram.contains(" my-app "));
    assert!(datagram.contains("local.INFO: Hello, Papertrail"));
}

#[cfg(unix)]
#[test]
fn syslog_handler_sends_to_the_local_daemon() {
    use illuminate_log::{Handler, LogRecord, SyslogHandler};
    use std::os::unix::net::UnixDatagram;

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("log.sock");
    let server = UnixDatagram::bind(&socket_path).unwrap();
    let handler = SyslogHandler::new("laravel", illuminate_log::syslog_facility(&json!("local0")))
        .with_socket_path(&socket_path);

    let record = LogRecord::new("production", Level::Error, "Database unavailable", Default::default());
    handler.handle(&record).unwrap();

    let mut buffer = [0u8; 2048];
    let length = server.recv(&mut buffer).unwrap();
    let message = String::from_utf8_lossy(&buffer[..length]).into_owned();
    assert!(message.starts_with("<131>"), "{message}");
    assert!(message.contains(" laravel["));
    assert!(message.ends_with("]: production.ERROR: Database unavailable [] []"));
}

// ----------------------------------------------------------------------
// Facade & helpers
// ----------------------------------------------------------------------

fn app_with_log(config: Value) -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
    let app = Arc::new(Container::new());
    let guard = Container::set_local_instance(app.clone());
    app.instance(Repository::new(config));
    LogServiceProvider.register(&app);
    (app, guard)
}

#[test]
fn the_facade_logs_through_the_container_manager() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("laravel.log");
    let (app, _guard) = app_with_log(json!({"app": {"env": "local"}, "logging": {
        "default": "stack",
        "channels": {"stack": {"driver": "stack", "channels": ["single"]}, "single": single(&path)},
    }}));

    Log::info("facade");
    Log::error_with("with context", json!({"id": 1}));
    Log::channel("single").debug("channel");
    Log::stack(["single"]).notice("stacked");
    Log::with_context(json!({"request-id": 9}));
    Log::warning("contextual");
    info("helper");
    logger().critical("logger helper");

    let contents = read(&path);
    for expected in [
        "local.INFO: facade  ",
        "local.ERROR: with context {\"id\":1} ",
        "local.DEBUG: channel  ",
        "local.NOTICE: stacked  ",
        "local.WARNING: contextual {\"request-id\":9} ",
        "local.INFO: helper {\"request-id\":9} ",
        "local.CRITICAL: logger helper {\"request-id\":9} ",
    ] {
        assert!(contents.contains(expected), "missing [{expected}] in:\n{contents}");
    }
    assert!(Arc::ptr_eq(&app.make::<LogManager>(), &logger()));
}

#[test]
fn the_facade_registers_a_manager_when_none_is_bound() {
    let app = Arc::new(Container::new());
    let _guard = Container::set_local_instance(app.clone());

    Log::info("nobody is listening");
    Log::listen(|_| {});

    assert!(app.bound::<LogManager>());
    assert_eq!(Log::get_default_driver(), None);
}

#[test]
fn facade_extend_and_listen() {
    let (_app, _guard) = app_with_log(json!({"logging": {"default": "memory", "channels": {
        "memory": {"driver": "memory"},
    }}}));
    let handler = Arc::new(TestHandler::new());
    let h = handler.clone();
    Log::extend("memory", move |_, _| Ok(Monolog::new("memory").with_handler_arc(h.clone())));
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = heard.clone();
    Log::listen(move |event| seen.lock().unwrap().push(event.message.clone()));

    Log::alert("Wake up!");
    Log::share_context(json!({"shared": true}));
    Log::debug("Shared");

    assert!(handler.has_record(Level::Alert, "Wake up!"));
    assert_eq!(handler.records()[1].context["shared"], true);
    assert_eq!(*heard.lock().unwrap(), vec!["Wake up!", "Shared"]);
    Log::without_context();
    Log::flush_shared_context();
    assert!(Log::shared_context().is_empty());
}

#[allow(dead_code)]
fn assert_paths_are_send(_: PathBuf) {}
