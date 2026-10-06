//! # Illuminate Foundation
//!
//! The application, its bootstrapping, the HTTP and console kernels, the
//! exception handler, Artisan's framework commands, and testing helpers.

pub mod application;
pub mod bootstrap;
pub mod defaults;
pub mod helpers;
pub mod inspiring;
pub mod providers;

pub use application::{Application, VERSION};
pub use bootstrap::ConfigFile;
pub use helpers::*;
pub use inspiring::Inspiring;
