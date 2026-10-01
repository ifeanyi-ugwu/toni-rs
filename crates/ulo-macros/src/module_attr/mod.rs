//! `#[module(..)]`: a `Module` impl whose `register` calls the value API (§4, §8.1).
//!
//! A unit struct is identified by type (`ModuleIdentity::of_type::<Self>()`); a struct with
//! fields is a configured module, identified by value (`ModuleIdentity::of_value(self)`, which
//! needs `Eq + Hash + Clone`), and every `Secret<_>` field is registered with `m.secret(..)`.
//! `register` never returns early: `expr?` lowers to `m.try_value(expr)`.
//!
//! `register` writes, in order: `global`, the secrets, the imports, the providers, the
//! controllers, the exports. Imports and providers keep the order written, which is part of
//! collection order.

mod args;
mod providers;

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Ident, Index, ItemStruct, Member, Type};

use crate::shared::ulo;

pub(crate) use args::{ExportEntry, ModuleArgs};
pub(crate) use providers::ProviderEntry;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: ModuleArgs = syn::parse2(attr)?;
    let item: ItemStruct = syn::parse2(item)?;
    expand_struct(args, item)
}

/// The `ModuleDef` parameter of the generated `register`. Mixed-site, so no name in a providers
/// expression can shadow it.
pub(crate) fn module_def_param() -> Ident {
    Ident::new("m", Span::mixed_site())
}

/// The struct as written, followed by its `Module` impl.
pub(crate) fn expand_struct(args: ModuleArgs, item: ItemStruct) -> syn::Result<TokenStream> {
    let ulo = ulo();
    let m = module_def_param();
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();

    let identity = if item.fields.is_empty() {
        quote!(#ulo::ModuleIdentity::of_type::<Self>())
    } else {
        let this = quote!(self);
        quote_spanned!(name.span()=> #ulo::ModuleIdentity::of_value(#this))
    };
    let global = args.global.then(|| quote!(#m.global();));
    let secrets = secret_fields(&item);
    let imports = args.imports.iter().map(|import| {
        quote_spanned! {import.span()=>
            #m.import(#import);
        }
    });
    let providers = args.providers.iter().map(ProviderEntry::lower);
    let controllers = args.controllers.iter().map(|controller| {
        quote_spanned! {controller.span()=>
            #m.controller::<#controller>();
        }
    });
    let exports = args.exports.iter().map(|export| match export {
        ExportEntry::Export(ty) => quote_spanned! {ty.span()=>
            #m.export::<#ty>();
        },
        ExportEntry::Reexport(ty) => quote_spanned! {ty.span()=>
            #m.reexport::<#ty>();
        },
    });

    Ok(quote! {
        #item

        impl #impl_generics #ulo::Module for #name #ty_generics #where_clause {
            fn identity(&self) -> #ulo::ModuleIdentity {
                #identity
            }

            #[allow(unused_variables)]
            fn register(&self, #m: &mut #ulo::ModuleDef<'_>) {
                #global
                #secrets
                #(#imports)*
                #(#providers)*
                #(#controllers)*
                #(#exports)*
            }
        }
    })
}

/// `m.secret(&self.<field>);` for each field whose type's last path segment is `Secret`.
pub(crate) fn secret_fields(item: &ItemStruct) -> TokenStream {
    let m = module_def_param();
    // `self` keeps the call-site span of the `&self` the generated `register` declares; the call
    // around it is spanned at the field, where a `Secret<T>` whose `T` is not `Display` fails.
    let this = quote!(self);
    let calls = item.fields.iter().enumerate().filter(|(_, field)| is_secret(&field.ty)).map(|(index, field)| {
        let member = match &field.ident {
            Some(ident) => Member::Named(ident.clone()),
            None => Member::Unnamed(Index { index: index as u32, span: field.ty.span() }),
        };
        quote_spanned! {field.ty.span()=>
            #m.secret(&#this.#member);
        }
    });
    quote!(#(#calls)*)
}

fn is_secret(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path.qself.is_none() && path.path.segments.last().is_some_and(|s| s.ident == "Secret"),
        _ => false,
    }
}
