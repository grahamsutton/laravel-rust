//! `#[derive(Authenticatable)]` — let an Eloquent model log in.

use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

use crate::paths;

pub fn derive(input: DeriveInput) -> syn::Result<TokenStream> {
    let paths = paths::database();
    let Some(root) = paths.umbrella else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(Authenticatable)] requires the `laravel` crate",
        ));
    };
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "#[derive(Authenticatable)] does not support generic models",
        ));
    }
    let ident = &input.ident;
    let eloquent = &paths.eloquent;
    let support = &paths.support;

    Ok(quote! {
        impl #root::auth::Authenticatable for #ident {
            fn auth_identifier_name(&self) -> &'static str {
                <Self as #eloquent::Model>::primary_key()
            }

            fn auth_identifier(&self) -> #support::Value {
                #eloquent::Model::get_key(self)
            }

            fn auth_password(&self) -> ::std::string::String {
                match #eloquent::Model::get_attribute(self, "password") {
                    #support::Value::String(password) => password,
                    _ => ::std::string::String::new(),
                }
            }

            fn set_auth_password(&mut self, hashed: &str) {
                let _ = #eloquent::Model::set_attribute(self, "password", #support::Value::String(hashed.into()));
            }

            fn remember_token(&self) -> ::std::option::Option<::std::string::String> {
                match #eloquent::Model::get_attribute(self, "remember_token") {
                    #support::Value::String(token) if !token.is_empty() => ::std::option::Option::Some(token),
                    _ => ::std::option::Option::None,
                }
            }

            fn set_remember_token(&mut self, token: &str) {
                let _ = #eloquent::Model::set_attribute(self, "remember_token", #support::Value::String(token.into()));
            }
        }

        #root::__private::inventory::submit! {
            #root::foundation::auth::AuthModel::of::<#ident>()
        }
    })
}
