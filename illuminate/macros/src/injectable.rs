//! `#[derive(Injectable)]` — zero-configuration resolution from the container.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericArgument, PathArguments, Type};

use crate::paths;

/// If the type is `Arc<T>`, return `T`.
fn arc_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let last = path.path.segments.last()?;
    if last.ident != "Arc" {
        return None;
    }
    match &last.arguments {
        PathArguments::AngleBracketed(args) => args.args.iter().find_map(|arg| match arg {
            GenericArgument::Type(inner) => Some(inner),
            _ => None,
        }),
        _ => None,
    }
}

pub fn derive(input: DeriveInput) -> syn::Result<TokenStream> {
    let container = paths::component("illuminate-container", "container");
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(ident, "#[derive(Injectable)] only supports structs"));
    };

    let resolve = |ty: &Type| match arc_inner(ty) {
        Some(inner) => quote!(container.make::<#inner>()),
        None => quote!(::core::default::Default::default()),
    };

    let body = match &data.fields {
        Fields::Named(named) => {
            let fields = named.named.iter().map(|f| {
                let name = f.ident.as_ref().expect("named field");
                let value = resolve(&f.ty);
                quote!(#name: #value)
            });
            quote!(Self { #(#fields),* })
        }
        Fields::Unnamed(unnamed) => {
            let fields = unnamed.unnamed.iter().map(|f| resolve(&f.ty));
            quote!(Self(#(#fields),*))
        }
        Fields::Unit => quote!(Self),
    };

    Ok(quote! {
        impl #impl_generics #container::Injectable for #ident #ty_generics #where_clause {
            fn inject(container: &#container::Container) -> Self {
                let _ = container;
                #body
            }
        }
    })
}
