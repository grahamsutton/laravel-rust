//! Resolving the paths generated code should use.
//!
//! Applications depend on the `laravel` crate, while the framework's own
//! crates (and their tests) depend on `illuminate-*` crates directly. We look
//! at the caller's manifest to emit paths that resolve in either world.

use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::Ident;

/// The paths generated code should use for framework items.
pub struct Paths {
    /// Path to the Eloquent module (`::laravel::database::eloquent`).
    pub eloquent: TokenStream,
    /// Path to the database crate root (`::laravel::database`).
    pub database: TokenStream,
    /// Path to the support crate (`::laravel::support`).
    pub support: TokenStream,
    /// Path to the routing + http integration, when available (only through
    /// the `laravel` umbrella crate).
    pub umbrella: Option<TokenStream>,
}

fn path_for(found: FoundCrate, crate_ident: &str) -> TokenStream {
    match found {
        FoundCrate::Itself => {
            // Doc tests are compiled as their own crates, with the
            // documented crate's environment.
            let compiling = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
            let doctest = std::env::var_os("UNSTABLE_RUSTDOC_TEST_PATH").is_some();
            if compiling == crate_ident && !doctest {
                quote!(crate)
            } else {
                let ident = Ident::new(crate_ident, Span::call_site());
                quote!(::#ident)
            }
        }
        FoundCrate::Name(name) => {
            let ident = Ident::new(&name, Span::call_site());
            quote!(::#ident)
        }
    }
}

/// Resolve the paths for database-related derives.
pub fn database() -> Paths {
    if let Ok(found) = crate_name("laravel") {
        let root = path_for(found, "laravel");
        return Paths {
            eloquent: quote!(#root::database::eloquent),
            database: quote!(#root::database),
            support: quote!(#root::support),
            umbrella: Some(root),
        };
    }
    let database = crate_name("illuminate-database")
        .map(|found| path_for(found, "illuminate_database"))
        .unwrap_or_else(|_| quote!(::illuminate_database));
    Paths {
        eloquent: quote!(#database::eloquent),
        support: quote!(#database::__private::support),
        database,
        umbrella: None,
    }
}

/// Resolve the path to the `laravel` crate, or to the given Illuminate crate.
pub fn component(illuminate_crate: &str, laravel_module: &str) -> TokenStream {
    if let Ok(found) = crate_name("laravel") {
        let root = path_for(found, "laravel");
        let module = Ident::new(laravel_module, Span::call_site());
        return quote!(#root::#module);
    }
    let ident = illuminate_crate.replace('-', "_");
    crate_name(illuminate_crate)
        .map(|found| path_for(found, &ident))
        .unwrap_or_else(|_| {
            let ident = Ident::new(&ident, Span::call_site());
            quote!(::#ident)
        })
}
