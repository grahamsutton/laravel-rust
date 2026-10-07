//! Procedural macros for the Laravel framework.
//!
//! You'll rarely use this crate directly: its macros are re-exported by the
//! `laravel` crate (and by `illuminate-database` for Eloquent).

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod authenticatable;
mod injectable;
mod model;
mod paths;

/// Turn a plain struct into an Eloquent model.
///
/// ```ignore
/// use laravel::prelude::*;
///
/// #[derive(Debug, Clone, Default, Model)]
/// #[fillable(name, email, password)]
/// #[hidden(password, remember_token)]
/// pub struct User {
///     pub id: u64,
///     pub name: String,
///     pub email: String,
///     #[hashed]
///     pub password: String,
///     pub remember_token: Option<String>,
///     pub created_at: Option<Carbon>,
///     pub updated_at: Option<Carbon>,
///
///     #[relation]
///     pub posts: Option<Vec<Post>>,
/// }
///
/// impl User {
///     pub fn posts(&self) -> HasMany<Self, Post> {
///         self.has_many()
///     }
/// }
/// ```
///
/// Conventions do the rest: the table is the snake-cased plural of the
/// struct's name (`users`), the primary key is `id`, and `created_at` /
/// `updated_at` are maintained automatically when present.
///
/// Struct attributes mirror Laravel's model attributes: `#[table("...")]`,
/// `#[connection("...")]`, `#[primary_key("...")]`, `#[fillable(...)]`,
/// `#[guarded(...)]`, `#[unguarded]`, `#[hidden(...)]`, `#[visible(...)]`,
/// `#[appends(...)]`, `#[without_incrementing]`, `#[without_timestamps]`,
/// `#[soft_deletes]`, `#[has_uuids]`, `#[has_ulids]`, `#[route_key("...")]`,
/// `#[per_page(25)]`, `#[use_factory(UserFactory)]`, and
/// `#[observed_by(UserObserver)]`. Field attributes: `#[relation]` (an
/// eager-loadable relationship whose loader is the method of the same name),
/// `#[hashed]` (hash the value when saving), `#[computed]` (read from queries
/// such as `with_count`, never saved) and `#[primary_key]`.
#[proc_macro_derive(
    Model,
    attributes(
        table,
        connection,
        primary_key,
        fillable,
        guarded,
        unguarded,
        hidden,
        visible,
        appends,
        without_incrementing,
        without_timestamps,
        soft_deletes,
        has_uuids,
        has_ulids,
        route_key,
        per_page,
        use_factory,
        observed_by,
        relation,
        computed,
        hashed
    )
)]
pub fn derive_model(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    model::derive(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Let the service container build a type by resolving its dependencies.
///
/// Every `Arc<T>` field is resolved from the container; every other field
/// uses its `Default`.
///
/// ```ignore
/// #[derive(Injectable)]
/// pub struct PodcastController {
///     podcasts: Arc<AppleMusic>,
/// }
///
/// let controller = app().make_or_build::<PodcastController>();
/// ```
#[proc_macro_derive(Injectable)]
pub fn derive_injectable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    injectable::derive(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Let an Eloquent model log in: `config/auth.rs` can then name it as the
/// `eloquent` provider's `model`.
///
/// ```ignore
/// #[derive(Debug, Clone, Default, Model, Authenticatable)]
/// #[hidden(password, remember_token)]
/// pub struct User {
///     pub id: u64,
///     pub email: String,
///     #[hashed]
///     pub password: String,
///     pub remember_token: Option<String>,
/// }
/// ```
#[proc_macro_derive(Authenticatable)]
pub fn derive_authenticatable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    authenticatable::derive(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
