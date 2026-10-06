//! # Illuminate Foundation
//!
//! The application, its bootstrapping, the HTTP and console kernels, the
//! exception handler, Artisan's framework commands, and testing helpers.

pub mod application;
pub mod bootstrap;
pub mod builder;
pub mod configuration;
pub mod defaults;
pub mod exceptions;
pub mod facade;
pub mod helpers;
pub mod http;
pub mod inspiring;
mod integration;
pub mod providers;
pub mod testing;

pub use application::{Application, VERSION};
pub use bootstrap::ConfigFile;
pub use builder::ApplicationBuilder;
pub use http::kernel::HttpKernel;
pub use facade::App;
pub use helpers::*;
pub use inspiring::Inspiring;
