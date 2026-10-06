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
//!
//! A quick tour:
//!
//! - Strings: [`Str`], the fluent [`Stringable`] (`str("...")`), the
//!   [`Pluralizer`], and PHP-style regular expressions in [`preg`].
//! - Arrays and data: [`Arr`], [`Fluent`], `data_get` / `data_set` /
//!   `data_fill` / `data_forget`, and the dynamic [`Value`].
//! - Collections: [`Collection`] (`collect(...)`) and [`LazyCollection`].
//! - Dates: [`Carbon`], [`CarbonInterval`] and [`CarbonPeriod`].
//! - Numbers: [`Number`].
//! - Odds and ends: [`Uri`], [`Js`], [`MessageBag`], [`ViewErrorBag`],
//!   [`Lottery`], [`Sleep`], [`Timebox`], [`Benchmark`] and [`once()`].
//!
//! Testing fakes (`Str::create_uuids_using`, `Sleep::fake`,
//! `Lottery::always_win`, `Carbon::with_test_now`, ...) apply to the current
//! thread only, so tests stay parallel-safe.

pub mod arr;
pub mod benchmark;
pub mod carbon;
pub mod collection;
pub mod env;
pub mod error;
pub mod fluent;
pub mod helpers;
pub mod html_string;
pub mod js;
pub mod lottery;
pub mod message_bag;
pub mod number;
pub mod once;
pub mod pluralizer;
pub mod preg;
pub mod sleep;
pub mod str;
pub mod stringable;
pub mod timebox;
pub mod traits;
pub mod uri;
mod transliteration;
pub mod value;
pub mod view_error_bag;

pub use arr::Arr;
pub use benchmark::Benchmark;
pub use carbon::{Carbon, CarbonImmutable, CarbonInterval, CarbonPeriod};
pub use collection::{Collection, LazyCollection};
pub use env::Env;
pub use error::{Error, Result};
pub use fluent::Fluent;
pub use helpers::*;
pub use html_string::HtmlString;
pub use js::Js;
pub use lottery::Lottery;
pub use message_bag::MessageBag;
pub use number::Number;
pub use once::{Once, once};
pub use pluralizer::Pluralizer;
pub use sleep::Sleep;
pub use str::Str;
pub use stringable::Stringable;
pub use timebox::Timebox;
pub use traits::{Conditionable, Tappable};
pub use uri::{Uri, UriQueryString};
pub use value::{Map, Number as JsonNumber, Value, ValueExt, cast, json, to_value};
pub use view_error_bag::ViewErrorBag;

/// Re-exports used by the framework's macros. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use serde;
    pub use serde_json;
}
