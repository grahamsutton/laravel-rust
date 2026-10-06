//! # Illuminate Validation
//!
//! Laravel's validator: dozens of expressive rules, wildcard array
//! validation, custom rule objects and closures, form requests, and
//! beautiful default error messages.
//!
//! ```
//! use illuminate_validation::{Rule, Validator, rules};
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
//! let mut validator = Validator::make(
//!     json!({
//!         "title": "Laravel for Rust",
//!         "tags": ["php", "go"],
//!         "author": {"email": "not-an-email"},
//!     }),
//!     rules! {
//!         "title" => "required|string|max:255",
//!         "tags.*" => ["string", Rule::in_(["php", "rust"])],
//!         "author.email" => "required|email",
//!     },
//! );
//!
//! assert!(validator.fails().await);
//! assert_eq!(validator.errors().first("tags.1"), Some("The selected tags.1 is invalid."));
//! assert_eq!(
//!     validator.errors().first("author.email"),
//!     Some("The author.email field must be a valid email address.")
//! );
//! # });
//! ```
//!
//! Validating an incoming request is just as pleasant:
//!
//! ```
//! use illuminate_http::Request;
//! use illuminate_validation::{ValidatesRequests, rules};
//! use illuminate_support::json;
//!
//! async fn store(request: Request) -> illuminate_support::Result<String> {
//!     let validated = request.validate(rules! {
//!         "title" => "required|unique:posts|max:255",
//!         "body" => "required",
//!     }).await?;
//!
//!     Ok(format!("Stored {}", validated["title"]))
//! }
//! ```

mod data;
mod date;
mod exception;
mod factory;
mod files;
mod formats;
mod http;
mod messages;
mod parser;
mod php;
mod presence;
mod rule;
mod rules;
mod validated_input;
mod validates;
mod validator;

pub use exception::{ErrorMessages, ValidationException};
pub use factory::{ExtensionFn, Factory, PendingExtension, ReplacerFn, ValidationServiceProvider};
pub use formats::InvalidPatternException;
pub use http::{
    FormRequest, Validated, ValidatedRequestData, ValidatesRequests, validate_form_request,
    validate_form_request_with,
};
pub use messages::{EnglishMessages, MessageResolver};
pub use presence::{ArrayPresenceVerifier, PresenceVerifier, split_table};
pub use rule::{
    ClosureRule, CurrentPasswordVerifier, FailCallback, UncompromisedVerifier, ValidationContext,
    ValidationRule,
};
pub use rules::{
    AnyOf, ArrayKeys, ArrayRule, BackedEnum, Condition, ConditionalRules, Contains, DataCondition,
    DatabaseRule, DatabaseRuleKind, DateArg, DateRule, Dimensions, DoesntContain, EmailRule, Enum,
    Exists, FileRule, FileSize, ImageFile, In, IntoCondition, IntoDataCondition, IntoRuleItems,
    NestedRules, NotIn, NumericRule, Password, PasswordDefaults, Rule, RuleItem, RuleSet, Rules,
    StringRule, Unique,
};
pub use validated_input::ValidatedInput;
pub use validator::{AfterHook, CustomAttributes, CustomMessages, IntoAttributeList, Validator};

/// Re-exported so rule objects can be implemented without adding a dependency.
pub use async_trait::async_trait;

/// `File::types([...])`, `File::image()` — Laravel's file rule builder by
/// its familiar name.
pub type File = FileRule;

/// Everything you need to validate, in one import.
pub mod prelude {
    pub use crate::{
        FailCallback, FormRequest, Password, Rule, Rules, Validated, ValidatedInput,
        ValidatesRequests, ValidationException, ValidationRule, Validator, async_trait, rules,
        validate_form_request,
    };
}
