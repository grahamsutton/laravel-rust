//! # Illuminate Foundation
//!
//! The application, its bootstrapping, the HTTP and console kernels, the
//! exception handler, Artisan's framework commands, and testing helpers.

pub mod application;
pub mod auth;
pub mod bootstrap;
pub mod builder;
pub mod configuration;
pub mod console;
pub mod defaults;
pub mod exceptions;
pub mod facade;
pub mod helpers;
pub mod http;
pub mod inspiring;
pub mod integration;
pub mod logging;
pub mod providers;
pub mod scheduling;
pub mod testing;
pub mod vite;

pub use application::{Application, VERSION};
pub use bootstrap::ConfigFile;
pub use builder::ApplicationBuilder;
pub use http::kernel::HttpKernel;
pub use facade::App;
pub use helpers::*;
pub use inspiring::Inspiring;
pub use vite::Vite;
