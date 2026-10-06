//! # Illuminate View
//!
//! Laravel's view layer and a native Rust implementation of the **Blade**
//! templating engine.
//!
//! Templates keep Blade's PHP-flavored syntax, so they feel right at home:
//!
//! ```blade
//! <!-- resources/views/greeting.blade.html -->
//! <h1>Hello, {{ $user->name }}</h1>
//!
//! @foreach ($user->roles as $role)
//!     <span @class(['badge', 'badge-admin' => $role === 'admin'])>{{ Str::title($role) }}</span>
//! @endforeach
//! ```
//!
//! Instead of compiling to PHP, Blade templates are parsed once into a tree
//! (cached by path and re-parsed only when the file changes) and evaluated
//! against the view's data.
//!
//! ```
//! use illuminate_view::Factory;
//! use illuminate_support::json;
//!
//! let factory = Factory::new(Vec::<String>::new());
//! let html = factory
//!     .render_inline(
//!         "@foreach ($users as $user){{ $loop->iteration }}. {{ $user['name'] }}@unless($loop->last), @endunless\n@endforeach",
//!         json!({"users": [{"name": "Taylor"}, {"name": "Abigail"}]}),
//!     )
//!     .unwrap();
//!
//! assert_eq!(html, "1. Taylor, 2. Abigail");
//! ```
//!
//! ## Views
//!
//! Views live in the directories configured by `view.paths`. Dot notation
//! maps to directories: `"layouts.app"` is `layouts/app.blade.html` (Blade
//! templates may also use `.blade.php`; plain `.html` files are served
//! as-is). Namespaces (`View::add_namespace("mail", path)`) make
//! `"mail::layout"` available.
//!
//! ## Framework hooks
//!
//! Blade directives that need application services call *functions* that
//! the framework registers with [`BladeCompiler::function`] (or the
//! [`facades::Blade`] facade). Registered functions always take precedence
//! over the built-in defaults listed here:
//!
//! | Function | Used by | Arguments | Built-in default |
//! | --- | --- | --- | --- |
//! | `csrf_token` | `csrf_token()`, `@csrf` | none | none (error) |
//! | `csrf_field` | `@csrf` | none | hidden `_token` input built from `csrf_token()` |
//! | `method_field` | `@method('PUT')` | method | hidden `_method` input |
//! | `auth_check` | `@auth`, `@guest`, `Auth::check()` | guard name (optional) | `false` |
//! | `auth` | `auth()`, `Auth::user()` / `Auth::id()` | guard name (optional) → object with `check`, `guest`, `user`, `id` | none |
//! | `gate_check` | `@can`, `@cannot`, `@canany`, `Gate::allows` | ability, then arguments | `false` |
//! | `app_environment` | `@env`, `@production`, `App::environment()` | none → current environment | `config('app.env')`, else `"production"` |
//! | `__` / `trans` | `@lang`, `__()` | key, replacements, locale | the key, with `:placeholders` replaced |
//! | `trans_choice` | `@choice`, `trans_choice()` | key, count, replacements, locale | pluralized key (`"apple\|apples"`) |
//! | `session` | `@session('key')`, `session()` | key, default | `null` / the default |
//! | `old` | `old()` | key, default | the default |
//! | `vite` | `@vite([...])` | entry points, build directory | empty |
//! | `vite_react_refresh` | `@viteReactRefresh` | none | empty |
//! | `app_locale` | `app()->getLocale()`, `App::getLocale()` | none → locale | `config('app.locale')`, else `"en"` |
//! | `app` | `@inject('name', 'service')`, `app()` | service name (none → the application) | no arguments: an [`AppObject`]; with a service: error |
//! | `config` | `config()` | key, default | the container's config repository |
//! | `request` | `request()` | key, default | the current request ([`RequestObject`]) |
//! | `route`, `url`, `asset`, `secure_asset`, `action`, ... | helpers | as in Laravel | none (error) |
//!
//! The defaults let templates render outside a full application (in tests,
//! say); the framework registers the real implementations at boot.
//!
//! Static calls look for a function registered as `"Class::method"` first,
//! so `Blade::function("Route::has", ...)` makes `Route::has('login')` work.
//! (`Auth::check()`, `Auth::user()`, `Gate::allows()`, `Config::get()`,
//! `Session::get()`, `Lang::get()`, `URL::to()` and `App::environment()`
//! fall back to the hooks above.)
//!
//! ## Whitespace
//!
//! Output matches Laravel's: a directive swallows the single newline that
//! directly follows it (as PHP does after `?>`), echoes keep theirs, and
//! component tags swallow the newline after them. Like Laravel, a directive
//! glued to a word (`foo@if`) is plain text — that's what keeps
//! `taylor@laravel.com` intact.
//!
//! The `$errors` variable is always defined. When the view data (or data
//! from a [`Factory::share_resolver`]) contains `errors` as JSON shaped like
//! `{"default": {"email": ["..."]}}` (or a flat `{"email": ["..."]}`), it is
//! wrapped in a [`ViewErrorBag`].

#![warn(missing_docs)]

pub mod attributes;
pub mod compiler;
pub mod component;
pub mod exception;
pub(crate) mod expr;
pub mod facades;
pub mod factory;
pub mod finder;
pub(crate) mod functions;
pub(crate) mod methods;
pub mod objects;
pub mod php;
pub mod provider;
pub(crate) mod registry;
pub(crate) mod render;
pub(crate) mod statics;
pub(crate) mod template;
pub mod value;
pub mod view;

pub use attributes::{AppendableAttributeValue, ComponentAttributeBag};
pub use compiler::BladeCompiler;
pub use component::{Component, ComponentArgs, ComponentSlot, ComponentView};
pub use exception::{ViewCompilationException, ViewException};
pub use facades::Blade;
pub use factory::{Factory, IntoViewData, ShareResolver, ViewCallback, ViewPatterns};
pub use finder::FileViewFinder;
pub use objects::{
    AppObject, DateObject, MessageBagObject, OptionalObject, RequestObject, ViewErrorBag,
};
pub use provider::ViewServiceProvider;
pub use registry::{ComponentFactory, ConditionHandler, DirectiveHandler};
pub use value::{
    ArrayKey, ViewArray, ViewClosure, ViewData, ViewFunction, ViewObject, ViewValue, data,
};
pub use view::{View, ViewInfo};

/// Get a view instance (the `view()` helper).
///
/// ```ignore
/// Route::get("/", || async { view("greeting", json!({"name": "James"})) });
/// ```
pub fn view(name: &str, data: impl IntoViewData) -> View {
    Factory::resolve().make(name, data)
}
