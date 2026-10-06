//! Exception handling: reporting errors and rendering them into responses.

mod handler;
mod pages;

pub use handler::{Exceptions, Handler, Prepared};
pub use pages::{render_debug_page, render_minimal_page};
