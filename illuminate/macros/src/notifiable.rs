//! `#[derive(Notifiable)]` — let an Eloquent model receive notifications.

use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

use crate::paths;

pub fn derive(input: DeriveInput) -> syn::Result<TokenStream> {
    let paths = paths::database();
    let Some(root) = paths.umbrella else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(Notifiable)] requires the `laravel` crate",
        ));
    };
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let eloquent = &paths.eloquent;
    let support = &paths.support;

    Ok(quote! {
        impl #impl_generics #root::notifications::Notifiable for #ident #ty_generics #where_clause {
            fn notifiable_key(&self) -> #support::Value {
                #eloquent::Model::get_key(self)
            }

            fn notifiable_type(&self) -> ::std::string::String {
                <Self as #eloquent::Model>::morph_class()
            }
        }
    })
}
