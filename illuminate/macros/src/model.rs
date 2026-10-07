//! `#[derive(Model)]` — Eloquent models from plain Rust structs.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Data, DeriveInput, Expr, Fields, Ident, Lit, LitStr, Meta, Path, Token, Type};

use crate::paths;

/// A list of names given as identifiers or string literals:
/// `#[fillable(name, email)]` or `#[fillable("name", "email")]`.
struct NameList(Vec<String>);

impl Parse for NameList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut names = Vec::new();
        while !input.is_empty() {
            if input.peek(LitStr) {
                names.push(input.parse::<LitStr>()?.value());
            } else {
                names.push(input.call(Ident::parse_any)?.unraw().to_string());
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(Self(names))
    }
}

#[derive(Default)]
struct ModelOptions {
    table: Option<String>,
    connection: Option<String>,
    primary_key: Option<String>,
    fillable: Vec<String>,
    guarded: Option<Vec<String>>,
    unguarded: bool,
    hidden: Vec<String>,
    visible: Vec<String>,
    appends: Vec<String>,
    without_incrementing: bool,
    without_timestamps: bool,
    soft_deletes: bool,
    has_uuids: bool,
    has_ulids: bool,
    route_key: Option<String>,
    per_page: Option<u64>,
    factory: Option<Path>,
    observers: Vec<Expr>,
}

struct FieldInfo {
    ident: Ident,
    name: String,
    ty: Type,
    relation: bool,
    computed: bool,
    hashed: bool,
    primary_key: bool,
}

/// Parse a single string argument: `#[table("users")]` or `#[table = "users"]`.
fn string_arg(meta: &Meta) -> syn::Result<String> {
    match meta {
        Meta::List(list) => Ok(list.parse_args::<LitStr>()?.value()),
        Meta::NameValue(nv) => match &nv.value {
            Expr::Lit(expr) => match &expr.lit {
                Lit::Str(s) => Ok(s.value()),
                other => Err(syn::Error::new_spanned(other, "expected a string")),
            },
            other => Err(syn::Error::new_spanned(other, "expected a string")),
        },
        Meta::Path(path) => Err(syn::Error::new_spanned(
            path,
            "expected a value, e.g. #[table(\"users\")]",
        )),
    }
}

fn list_arg(meta: &Meta) -> syn::Result<Vec<String>> {
    match meta {
        Meta::List(list) => Ok(list.parse_args::<NameList>()?.0),
        other => Err(syn::Error::new_spanned(
            other,
            "expected a list, e.g. #[hidden(password)]",
        )),
    }
}

fn parse_options(input: &DeriveInput) -> syn::Result<ModelOptions> {
    let mut options = ModelOptions::default();
    for attr in &input.attrs {
        let Some(name) = attr.path().get_ident().map(|i| i.to_string()) else {
            continue;
        };
        let meta = &attr.meta;
        match name.as_str() {
            "table" => options.table = Some(string_arg(meta)?),
            "connection" => options.connection = Some(string_arg(meta)?),
            "primary_key" => options.primary_key = Some(string_arg(meta)?),
            "route_key" => options.route_key = Some(string_arg(meta)?),
            "fillable" => options.fillable.extend(list_arg(meta)?),
            "guarded" => options
                .guarded
                .get_or_insert_with(Vec::new)
                .extend(list_arg(meta)?),
            "hidden" => options.hidden.extend(list_arg(meta)?),
            "visible" => options.visible.extend(list_arg(meta)?),
            "appends" => options.appends.extend(list_arg(meta)?),
            "unguarded" => options.unguarded = true,
            "without_incrementing" => options.without_incrementing = true,
            "without_timestamps" => options.without_timestamps = true,
            "soft_deletes" => options.soft_deletes = true,
            "has_uuids" => options.has_uuids = true,
            "has_ulids" => options.has_ulids = true,
            "per_page" => {
                let Meta::List(list) = meta else {
                    return Err(syn::Error::new_spanned(meta, "expected #[per_page(25)]"));
                };
                let lit: syn::LitInt = list.parse_args()?;
                options.per_page = Some(lit.base10_parse()?);
            }
            "use_factory" => {
                let Meta::List(list) = meta else {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "expected #[use_factory(UserFactory)]",
                    ));
                };
                options.factory = Some(list.parse_args::<Path>()?);
            }
            "observed_by" => {
                let Meta::List(list) = meta else {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "expected #[observed_by(UserObserver)]",
                    ));
                };
                let observers =
                    list.parse_args_with(Punctuated::<Expr, Token![,]>::parse_terminated)?;
                options.observers.extend(observers);
            }
            _ => {}
        }
    }
    Ok(options)
}

fn parse_fields(input: &DeriveInput) -> syn::Result<Vec<FieldInfo>> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(Model)] only supports structs",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(Model)] requires named fields",
        ));
    };
    let mut fields = Vec::new();
    for field in &named.named {
        let ident = field.ident.clone().expect("named field");
        let mut info = FieldInfo {
            name: ident.unraw().to_string(),
            ident,
            ty: field.ty.clone(),
            relation: false,
            computed: false,
            hashed: false,
            primary_key: false,
        };
        for attr in &field.attrs {
            match attr.path().get_ident().map(|i| i.to_string()).as_deref() {
                Some("relation") => info.relation = true,
                Some("computed") => info.computed = true,
                Some("hashed") => info.hashed = true,
                Some("primary_key") => info.primary_key = true,
                _ => {}
            }
        }
        fields.push(info);
    }
    Ok(fields)
}

/// Does the type look like a string key (String, Uuid, Ulid, or an Option of one)?
fn is_string_key(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    let Some(last) = path.path.segments.last() else {
        return false;
    };
    match last.ident.to_string().as_str() {
        "String" | "Uuid" | "Ulid" | "str" => true,
        "Option" => match &last.arguments {
            syn::PathArguments::AngleBracketed(args) => args.args.iter().any(|arg| match arg {
                syn::GenericArgument::Type(inner) => is_string_key(inner),
                _ => false,
            }),
            _ => false,
        },
        _ => false,
    }
}

fn str_slice(items: &[String]) -> TokenStream {
    quote!(&[#(#items),*])
}

pub fn derive(input: DeriveInput) -> syn::Result<TokenStream> {
    let options = parse_options(&input)?;
    let fields = parse_fields(&input)?;
    let paths = paths::database();
    let eloquent = &paths.eloquent;
    let support = &paths.support;

    let ident = &input.ident;
    let class_name = ident.unraw().to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let primary_key = options
        .primary_key
        .clone()
        .or_else(|| {
            fields
                .iter()
                .find(|f| f.primary_key)
                .map(|f| f.name.clone())
        })
        .unwrap_or_else(|| "id".to_string());
    let key_field = fields.iter().find(|f| f.name == primary_key);
    let string_key =
        options.has_uuids || options.has_ulids || key_field.is_some_and(|f| is_string_key(&f.ty));

    let table = match &options.table {
        Some(table) => quote!(::std::string::String::from(#table)),
        None => quote! {
            static TABLE: ::std::sync::OnceLock<::std::string::String> = ::std::sync::OnceLock::new();
            TABLE.get_or_init(|| #eloquent::__private::table_name_for(#class_name)).clone()
        },
    };
    let connection = match &options.connection {
        Some(c) => quote!(::core::option::Option::Some(#c)),
        None => quote!(::core::option::Option::None),
    };

    let persisted: Vec<&FieldInfo> = fields
        .iter()
        .filter(|f| !f.relation && !f.computed)
        .collect();
    let readable: Vec<&FieldInfo> = fields.iter().filter(|f| !f.relation).collect();
    let relations: Vec<&FieldInfo> = fields.iter().filter(|f| f.relation).collect();

    let has_created_at = persisted.iter().any(|f| f.name == "created_at");
    let has_updated_at = persisted.iter().any(|f| f.name == "updated_at");
    let timestamps = !options.without_timestamps && (has_created_at || has_updated_at);
    let incrementing = !options.without_incrementing && !string_key;
    let key_type = if string_key {
        quote!(#eloquent::KeyType::String)
    } else {
        quote!(#eloquent::KeyType::Int)
    };
    let unique_ids = if options.has_uuids {
        quote!(#eloquent::UniqueIds::Uuid)
    } else if options.has_ulids {
        quote!(#eloquent::UniqueIds::Ulid)
    } else {
        quote!(#eloquent::UniqueIds::None)
    };

    let columns: Vec<String> = persisted.iter().map(|f| f.name.clone()).collect();
    let relation_names: Vec<String> = relations.iter().map(|f| f.name.clone()).collect();
    let hashed: Vec<String> = persisted
        .iter()
        .filter(|f| f.hashed)
        .map(|f| f.name.clone())
        .collect();
    let fillable = str_slice(&options.fillable);
    let guarded = if options.unguarded {
        quote!(&[])
    } else {
        match &options.guarded {
            Some(list) => str_slice(list),
            None if options.fillable.is_empty() => quote!(&["*"]),
            None => quote!(&[]),
        }
    };
    let hidden = str_slice(&options.hidden);
    let visible = str_slice(&options.visible);
    let appends = str_slice(&options.appends);
    let per_page = options.per_page.unwrap_or(15);
    let route_key = options
        .route_key
        .clone()
        .unwrap_or_else(|| primary_key.clone());
    let columns_tokens = str_slice(&columns);
    let relation_tokens = str_slice(&relation_names);
    let hashed_tokens = str_slice(&hashed);
    let soft_deletes = options.soft_deletes;

    // to_attributes: persisted fields, in storage format.
    let to_attributes = persisted.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        quote!(attributes.insert(::std::string::String::from(#name), #support::to_value(&self.#ident));)
    });

    // from_attributes: every non-relation field.
    let from_fields = fields.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        if f.relation {
            quote!(#ident: ::core::default::Default::default())
        } else {
            quote!(#ident: #eloquent::__private::take_attribute(&mut attributes, #name, #class_name)?)
        }
    });

    let template_fields = fields.iter().map(|f| {
        let ident = &f.ident;
        quote!(#ident: ::core::default::Default::default())
    });

    // to_array: respects hidden / visible, appends, and loaded relations.
    let array_fields = readable.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        quote! {
            if #eloquent::__private::is_visible(#name, Self::hidden(), Self::visible()) {
                array.insert(::std::string::String::from(#name), #support::to_value(&self.#ident));
            }
        }
    });
    let append_fields = options.appends.iter().map(|name| {
        let method = format_ident!("{}", name);
        quote! {
            if #eloquent::__private::is_visible(#name, Self::hidden(), Self::visible()) {
                array.insert(::std::string::String::from(#name), #support::to_value(&self.#method()));
            }
        }
    });
    let relation_array = relations.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        quote! {
            if #eloquent::__private::is_visible(#name, Self::hidden(), Self::visible())
                && #eloquent::RelationValue::is_loaded(&self.#ident)
            {
                array.insert(::std::string::String::from(#name), #eloquent::RelationValue::to_value(&self.#ident));
            }
        }
    });

    let get_arms = readable.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        quote!(#name => #support::Carbon::with_storage_format(|| #support::to_value(&self.#ident)),)
    });
    let get_append_arms = options.appends.iter().map(|name| {
        let method = format_ident!("{}", name);
        quote!(#name => #support::to_value(&self.#method()),)
    });
    let set_arms = readable.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        quote! {
            #name => {
                self.#ident = #eloquent::__private::cast_attribute(value, #name, #class_name)?;
                ::core::result::Result::Ok(())
            }
        }
    });

    let eager_arms = relations.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        quote! {
            #name => {
                let relation = models[0].#ident();
                let loaded = #eloquent::Relation::eager_load(relation, &*models, spec).await?;
                for (model, value) in models.iter_mut().zip(loaded) {
                    model.#ident = #eloquent::FromEager::from_eager(value);
                }
                ::core::result::Result::Ok(())
            }
        }
    });
    let relation_arms = relations.iter().map(|f| {
        let ident = &f.ident;
        let name = &f.name;
        quote! {
            #name => ::core::option::Option::Some(#eloquent::Relation::into_dyn(Self::template().#ident())),
        }
    });

    let loaded_relations = if relations.is_empty() {
        quote!(::std::vec::Vec::new())
    } else {
        let checks = relations.iter().map(|f| {
            let ident = &f.ident;
            let name = &f.name;
            quote! {
                if #eloquent::RelationValue::is_loaded(&self.#ident) {
                    loaded.push(#name);
                }
            }
        });
        quote! {
            let mut loaded = ::std::vec::Vec::new();
            #(#checks)*
            loaded
        }
    };

    let observers = &options.observers;
    let boot = quote! {
        fn boot() {
            #( <Self as #eloquent::Model>::observe(#observers); )*
        }
    };

    let factory_impl = options.factory.as_ref().map(|factory| {
        quote! {
            impl #impl_generics #eloquent::HasFactory for #ident #ty_generics #where_clause {
                type Factory = #factory;
            }
        }
    });

    let routing_impls = paths.umbrella.as_ref().map(|root| {
        quote! {
            #[#root::async_trait]
            impl #impl_generics #root::routing::FromRequest for #ident #ty_generics #where_clause {
                async fn from_request(request: &#root::http::Request) -> #root::support::Result<Self> {
                    #root::__private::resolve_route_binding::<Self>(request).await
                }
            }

            impl #impl_generics #root::routing::UrlRoutable for #ident #ty_generics #where_clause {
                fn route_key(&self) -> ::std::string::String {
                    #eloquent::Model::route_key_value(self)
                }

                fn route_key_name() -> &'static str {
                    <Self as #eloquent::Model>::route_key_name()
                }
            }
        }
    });

    let serde = quote!(#eloquent::__private::serde);

    Ok(quote! {
        impl #impl_generics #eloquent::Model for #ident #ty_generics #where_clause {
            fn table() -> ::std::string::String { #table }
            fn class_name() -> &'static str { #class_name }
            fn primary_key() -> &'static str { #primary_key }
            fn key_type() -> #eloquent::KeyType { #key_type }
            fn incrementing() -> bool { #incrementing }
            fn timestamps() -> bool { #timestamps }
            fn uses_created_at() -> bool { #has_created_at }
            fn uses_updated_at() -> bool { #has_updated_at }
            fn connection_name() -> ::core::option::Option<&'static str> { #connection }
            fn fillable() -> &'static [&'static str] { #fillable }
            fn guarded() -> &'static [&'static str] { #guarded }
            fn hidden() -> &'static [&'static str] { #hidden }
            fn visible() -> &'static [&'static str] { #visible }
            fn appends() -> &'static [&'static str] { #appends }
            fn per_page() -> u64 { #per_page }
            fn route_key_name() -> &'static str { #route_key }
            fn soft_deletes() -> bool { #soft_deletes }
            fn unique_ids() -> #eloquent::UniqueIds { #unique_ids }
            fn hashed_attributes() -> &'static [&'static str] { #hashed_tokens }
            fn columns() -> &'static [&'static str] { #columns_tokens }
            fn relation_names() -> &'static [&'static str] { #relation_tokens }

            fn template() -> Self {
                Self { #(#template_fields),* }
            }

            fn to_attributes(&self) -> #support::Map<::std::string::String, #support::Value> {
                #support::Carbon::with_storage_format(|| {
                    let mut attributes = #support::Map::new();
                    #(#to_attributes)*
                    attributes
                })
            }

            fn from_attributes(
                mut attributes: #support::Map<::std::string::String, #support::Value>,
            ) -> #support::Result<Self> {
                ::core::result::Result::Ok(Self { #(#from_fields),* })
            }

            fn to_array(&self) -> #support::Map<::std::string::String, #support::Value> {
                let mut array = #support::Map::new();
                #(#array_fields)*
                #(#append_fields)*
                #(#relation_array)*
                array
            }

            fn get_attribute(&self, key: &str) -> #support::Value {
                match key {
                    #(#get_arms)*
                    #(#get_append_arms)*
                    _ => #support::Value::Null,
                }
            }

            fn set_attribute(&mut self, key: &str, value: #support::Value) -> #support::Result<()> {
                match key {
                    #(#set_arms)*
                    _ => ::core::result::Result::Ok(()),
                }
            }

            fn eager_load<'a>(
                models: &'a mut [Self],
                relation: &'a str,
                spec: #eloquent::EagerSpec,
            ) -> #eloquent::BoxFuture<'a, #support::Result<()>> {
                ::std::boxed::Box::pin(async move {
                    if models.is_empty() {
                        return ::core::result::Result::Ok(());
                    }
                    let _ = &spec;
                    match relation {
                        #(#eager_arms)*
                        other => ::core::result::Result::Err(
                            #eloquent::RelationNotFoundException::new(#class_name, other).into(),
                        ),
                    }
                })
            }

            fn relation(name: &str) -> ::core::option::Option<::std::boxed::Box<dyn #eloquent::DynRelation>> {
                match name {
                    #(#relation_arms)*
                    _ => ::core::option::Option::None,
                }
            }

            fn loaded_relations(&self) -> ::std::vec::Vec<&'static str> {
                #loaded_relations
            }

            #boot
        }

        impl #impl_generics #serde::Serialize for #ident #ty_generics #where_clause {
            fn serialize<S: #serde::Serializer>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error> {
                #serde::Serialize::serialize(&#eloquent::Model::to_array(self), serializer)
            }
        }

        #factory_impl

        #routing_impls
    })
}
