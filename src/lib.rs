//! # Laravel
//!
//! The PHP framework for web artisans — now in Rust.
//!
//! Laravel is a web application framework with expressive, elegant syntax.
//! This crate is the single dependency your application needs: it brings
//! every Illuminate component together, along with the prelude, facades, and
//! helpers you know.
//!
//! ```ignore
//! use laravel::prelude::*;
//!
//! pub fn routes() {
//!     Route::get("/", || async {
//!         view("welcome", ())
//!     });
//!
//!     Route::get("/users/{user}", |user: User| async move {
//!         Json(user)
//!     });
//! }
//! ```

// ---------------------------------------------------------------------------
// The Illuminate components
// ---------------------------------------------------------------------------

pub use illuminate_auth as auth;
pub use illuminate_broadcasting as broadcasting;
pub use illuminate_cache as cache;
pub use illuminate_config as config;
pub use illuminate_console as console;
pub use illuminate_container as container;
pub use illuminate_concurrency as concurrency;
pub use illuminate_cookie as cookie;
pub use illuminate_database::eloquent;
pub use illuminate_database as database;
pub use illuminate_encryption as encryption;
pub use illuminate_events as events;
pub use illuminate_filesystem as filesystem;
pub use illuminate_foundation as foundation;
pub use illuminate_hashing as hashing;
pub use illuminate_http as http;
pub use illuminate_http_client as http_client;
pub use illuminate_http_resources as http_resources;
pub use illuminate_log as log;
pub use illuminate_mail as mail;
pub use illuminate_notifications as notifications;
pub use illuminate_pagination as pagination;
pub use illuminate_pipeline as pipeline;
pub use illuminate_process as process;
pub use illuminate_queue as queue;
pub use illuminate_redis as redis;
pub use illuminate_routing as routing;
pub use illuminate_session as session;
pub use illuminate_support as support;
pub use illuminate_translation as translation;
pub use illuminate_validation as validation;
pub use illuminate_view as view;

/// Laravel Sanctum: API tokens and SPA authentication (the `sanctum` feature).
#[cfg(feature = "sanctum")]
pub use laravel_sanctum as sanctum;

/// Testing helpers: `TestApp`, `TestResponse`, and friends.
pub mod testing {
    pub use illuminate_foundation::testing::*;
}

/// The derive macros: `#[derive(Model)]`, `#[derive(Injectable)]`.
pub use illuminate_database::eloquent::Model;
pub use illuminate_macros::{Authenticatable, Injectable, Notifiable};

pub use async_trait::async_trait;
pub use illuminate_foundation::{Application, ApplicationBuilder, ConfigFile, Inspiring};
pub use illuminate_support::{Error, Result, Value, json};

// ---------------------------------------------------------------------------
// Facades
// ---------------------------------------------------------------------------

/// Laravel's facades: static, expressive access to framework services.
pub mod facades {
    pub use illuminate_auth::facades::{Auth, Gate, Password};
    pub use illuminate_broadcasting::Broadcast;
    pub use illuminate_cache::facades::{Cache, RateLimiter};
    pub use illuminate_config::Config;
    pub use illuminate_console::{Artisan, Schedule};
    pub use illuminate_cookie::facades::Cookie;
    pub use illuminate_database::{DB, Schema};
    pub use illuminate_encryption::Crypt;
    pub use illuminate_events::Event;
    pub use illuminate_filesystem::facades::{File, Storage};
    pub use illuminate_foundation::{App, Vite};
    pub use illuminate_hashing::Hash;
    pub use illuminate_log::{Context, Log};
    pub use illuminate_mail::Mail;
    pub use illuminate_notifications::facades::Notification;
    pub use illuminate_concurrency::Concurrency;
    pub use illuminate_http_client::Http;
    pub use illuminate_process::Process;
    pub use illuminate_queue::{Bus, Queue};
    pub use illuminate_redis::Redis;
    pub use illuminate_routing::{Redirect, Route, URL};
    pub use illuminate_session::Session;
    pub use illuminate_translation::Lang;
    pub use illuminate_validation::Validator;
    pub use illuminate_view::facades::View;
    pub use illuminate_view::Blade;
}

// ---------------------------------------------------------------------------
// Global helpers
// ---------------------------------------------------------------------------

/// Laravel's global helper functions.
pub mod helpers {
    pub use illuminate_auth::{auth, authorize};
    pub use illuminate_cache::cache;
    pub use illuminate_config::{config, config_or};
    pub use illuminate_container::{app, resolve, try_app};
    pub use illuminate_cookie::cookie;
    pub use illuminate_encryption::{decrypt, encrypt};
    pub use illuminate_events::event;
    pub use illuminate_foundation::{
        app_path, base_path, bootstrap_path, config_path, database_path, defer, lang_path, public_path,
        report, rescue, resource_path, storage_path,
    };
    pub use illuminate_hashing::bcrypt;
    pub use illuminate_http::{abort, abort_if, abort_unless, abort_with, request, response};
    pub use illuminate_log::{info, logger};
    pub use illuminate_queue::{dispatch, dispatch_sync};
    pub use illuminate_routing::{asset, back, redirect, route, secure_asset, secure_url, to_route, url};
    pub use illuminate_session::{
        csrf_field, csrf_token, method_field, old, redirect_guest, redirect_intended, session,
    };
    pub use illuminate_support::{
        blank, class_basename, collect, data_get, data_set, e, env, filled, now, retry, str, tap,
        throw_if, throw_unless, today, with,
    };
    pub use illuminate_translation::{__, trans, trans_choice};
    pub use illuminate_view::view;
}

// ---------------------------------------------------------------------------
// The prelude
// ---------------------------------------------------------------------------

/// Everything you need to build an application: `use laravel::prelude::*;`
pub mod prelude {
    pub use crate::facades::*;
    pub use crate::helpers::*;

    pub use async_trait::async_trait;
    pub use illuminate_database::eloquent::{
        BelongsTo, BelongsToMany, Builder, EloquentCollection, Factory, Faker, HasFactory, HasMany, HasManyThrough,
        HasOne, HasOneThrough, MassPrunable, Model, ModelNotFoundException, MorphMany, MorphOne, MorphTo, Observer,
        Original, Prunable, Scope,
    };
    pub use illuminate_macros::{Authenticatable, Injectable, Notifiable};
    pub use illuminate_mail::{Address, Attachment, Content, Envelope, Mailable};
    pub use illuminate_notifications::{MailMessage, Notifiable, Notification};
    pub use serde::{Deserialize, Serialize};

    pub use illuminate_container::{Container, Injectable as InjectableContract, ServiceProvider};
    pub use illuminate_filesystem::UploadedFileExt;
    pub use illuminate_foundation::{Application, ApplicationBuilder, ConfigFile, Inspiring};
    pub use illuminate_foundation::scheduling::ScheduleJobs;
    pub use illuminate_http::{
        HttpException, IntoResponse, Json, Middleware, Next, Request, Response, StatusCode,
        UploadedFile,
    };
    pub use illuminate_broadcasting::{
        Channel, EncryptedPrivateChannel, PresenceChannel, PrivateChannel, ShouldBroadcast, broadcast,
    };
    pub use illuminate_http_resources::prelude::*;
    pub use illuminate_pagination::{LengthAwarePaginator, Paginator};
    pub use illuminate_queue::{Dispatchable, InteractsWithQueue, ShouldQueue};
    pub use illuminate_routing::{
        FromRequest, Inject, Input, Path, Query, ResourceController, RoutingRequestExt, UrlRoutable,
    };
    pub use illuminate_auth::{AuthResponse, AuthUser, Authenticatable, MustVerifyEmail, Policy, RequestAuthExt};
    pub use illuminate_console::{Command, Console};
    pub use illuminate_database::{Blueprint, Migration, Seeder};
    pub use illuminate_session::RequestSessionExt;
    pub use illuminate_view::{Component, View};
    pub use illuminate_support::error::Context as _;
    pub use illuminate_validation::{
        FormRequest, Password, Rule, Rules, Validated, ValidatesRequests, ValidationException,
        ValidationRule, rules,
    };
    pub use illuminate_support::{
        Arr, Carbon, CarbonInterval, Collection, Conditionable, Error, HtmlString,
        Map, MessageBag, Number, Result, Str, Stringable, Tappable, Value, ValueExt, cast, json,
        to_value,
    };
}

/// Register job types so queue workers can run them: `register_job!(ProcessPodcast);`
pub use illuminate_queue::register_job;

/// Register events broadcast when dispatched: `register_broadcast!(OrderShipped);`
pub use illuminate_broadcasting::register_broadcast;

// ---------------------------------------------------------------------------
// Application conventions
// ---------------------------------------------------------------------------

/// The base path of your application (the directory holding `Cargo.toml`).
#[macro_export]
macro_rules! base_path {
    () => {
        env!("CARGO_MANIFEST_DIR")
    };
}

/// Include the migrations discovered in `database/migrations` by
/// `laravel_build::discover()`.
#[macro_export]
macro_rules! discover_migrations {
    () => {
        include!(concat!(env!("OUT_DIR"), "/laravel/migrations.rs"));
    };
}

/// Include the seeders discovered in `database/seeders`.
#[macro_export]
macro_rules! discover_seeders {
    () => {
        include!(concat!(env!("OUT_DIR"), "/laravel/seeders.rs"));
    };
}

/// Include the Artisan commands discovered in `app/console/commands`.
#[macro_export]
macro_rules! discover_commands {
    () => {
        include!(concat!(env!("OUT_DIR"), "/laravel/commands.rs"));
    };
}

/// Include the Blade components discovered in `app/view/components`.
#[macro_export]
macro_rules! discover_components {
    () => {
        include!(concat!(env!("OUT_DIR"), "/laravel/components.rs"));
    };
}

/// Include the policies discovered in `app/policies`.
#[macro_export]
macro_rules! discover_policies {
    () => {
        include!(concat!(env!("OUT_DIR"), "/laravel/policies.rs"));
    };
}

/// List the application's configuration files: `config_files![app, database]`
/// expects modules exposing `pub fn config() -> Value`.
#[macro_export]
macro_rules! config_files {
    ($($name:ident),* $(,)?) => {
        $(pub mod $name;)*

        /// Every configuration file in `config/`.
        pub fn all() -> ::std::vec::Vec<$crate::ConfigFile> {
            ::std::vec![$($crate::ConfigFile::new(stringify!($name), $name::config)),*]
        }
    };
}

/// Re-exports used by the framework's macros. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use inventory;
    pub use serde;
    pub use serde_json;

    use illuminate_database::eloquent::{Model, ModelNotFoundException};
    use illuminate_http::Request;
    use illuminate_support::{Result, Str, Value};

    /// Route model binding: resolve `{user}` (or `{user:email}`) into a `User`.
    pub async fn resolve_route_binding<M: Model>(request: &Request) -> Result<M> {
        let name = Str::snake(M::class_name());
        let Some((value, field)) = illuminate_routing::route_parameter_for(request, &name) else {
            return Err(illuminate_support::error::error!(
                "Unable to bind [{}]: the route has no {{{name}}} parameter.",
                M::class_name()
            ));
        };

        if M::soft_deletes() && illuminate_routing::route_allows_trashed_bindings(request) {
            return M::resolve_soft_deletable_route_binding(&value, field.as_deref())
                .await?
                .ok_or_else(|| ModelNotFoundException::new(M::class_name(), vec![Value::String(value)]).into());
        }

        M::resolve_route_binding_or_fail(&value, field.as_deref()).await
    }
}
