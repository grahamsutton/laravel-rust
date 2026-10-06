//! # Illuminate Support
//!
//! The foundation everything else in Laravel stands on: fluent strings,
//! array helpers, collections, dates, and the little helper functions that
//! make writing an application a joy.
//!
//! ```
//! use illuminate_support::{collect, Str};
//!
//! let names = collect(vec!["taylor", "abigail", "james"])
//!     .map(|name| Str::ucfirst(name))
//!     .implode(", ");
//!
//! assert_eq!(names, "Taylor, Abigail, James");
//! ```

pub mod arr;
pub mod carbon;
pub mod collection;
pub mod env;
pub mod error;
pub mod fluent;
pub mod helpers;
pub mod html_string;
pub mod message_bag;
pub mod number;
pub mod pluralizer;
pub mod preg;
pub mod str;
pub mod stringable;
pub mod traits;
pub mod uri;
mod transliteration;
pub mod value;

pub use arr::Arr;
pub use carbon::{Carbon, CarbonInterval};
pub use collection::Collection;
pub use env::Env;
pub use error::{Error, Result};
pub use fluent::Fluent;
pub use helpers::*;
pub use html_string::HtmlString;
pub use message_bag::MessageBag;
pub use number::Number;
pub use pluralizer::Pluralizer;
pub use str::Str;
pub use stringable::Stringable;
pub use traits::{Conditionable, Tappable};
pub use uri::{Uri, UriQueryString};
pub use value::{Map, Number as JsonNumber, Value, ValueExt, cast, json, to_value};

/// Re-exports used by the framework's macros. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use serde;
    pub use serde_json;
}
