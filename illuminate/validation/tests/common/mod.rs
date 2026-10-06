#![allow(dead_code)]

use std::sync::Arc;

use illuminate_container::{Container, LocalInstanceGuard};
use illuminate_support::{MessageBag, Value};
use illuminate_validation::{Rules, Validator};

/// Install a fresh container for the current test thread.
pub fn container() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    (container, guard)
}

/// Run a validator and return its error bag.
pub async fn errors(data: Value, rules: impl Into<Rules>) -> MessageBag {
    let mut validator = Validator::make(data, rules);
    validator.passes().await;
    validator.errors().clone()
}

/// Determine if the data passes the rules.
pub async fn passes(data: Value, rules: impl Into<Rules>) -> bool {
    Validator::make(data, rules).passes().await
}

/// Assert the data passes a single rule string for the `field` attribute.
pub async fn assert_passes(data: Value, rule: &str) {
    let mut validator = Validator::make(data.clone(), [("field", rule)]);
    let passed = validator.passes().await;
    assert!(
        passed,
        "expected {data} to pass [{rule}], got {:?}",
        validator.errors().all()
    );
}

/// Assert the data fails a single rule string for the `field` attribute
/// with exactly the given message.
pub async fn assert_fails(data: Value, rule: &str, message: &str) {
    let mut validator = Validator::make(data.clone(), [("field", rule)]);
    let passed = validator.passes().await;
    assert!(!passed, "expected {data} to fail [{rule}]");
    assert_eq!(
        validator.errors().first("field"),
        Some(message),
        "unexpected message for {data} with [{rule}]"
    );
}

/// Assert a value of `field` passes the rule.
pub async fn ok(value: Value, rule: &str) {
    assert_passes(illuminate_support::json!({ "field": value }), rule).await;
}

/// Assert a value of `field` fails the rule (any message).
pub async fn bad(value: Value, rule: &str) {
    let data = illuminate_support::json!({ "field": value });
    let mut validator = Validator::make(data.clone(), [("field", rule)]);
    assert!(
        !validator.passes().await,
        "expected {data} to fail [{rule}]"
    );
}
