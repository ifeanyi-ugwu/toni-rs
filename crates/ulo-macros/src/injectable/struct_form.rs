//! The struct form: every field is a site, read in declaration order, except a field marked
//! `#[injectable(default)]`, set from `Default::default()`. Tuple and unit structs are accepted;
//! a tuple field's site is labelled by its index.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Field, Index, ItemStruct, Member, Type};

use crate::injectable::InjectableArgs;
use crate::shared::attrs;
use crate::shared::sites::{self, SiteLabel, SiteSpec};
use crate::shared::{ConstructImpl, combine, ulo};

/// The struct as written, minus its `#[injectable(default)]` field markers, followed by its
/// `Construct` impl.
pub(crate) fn expand(args: InjectableArgs, mut item: ItemStruct) -> syn::Result<TokenStream> {
    let ulo = ulo();
    let mut site_specs = Vec::new();
    let mut site_members = Vec::new();
    let mut default_members = Vec::new();
    let mut errors = Vec::new();

    for (index, field) in item.fields.iter_mut().enumerate() {
        let member = match &field.ident {
            Some(ident) => Member::Named(ident.clone()),
            None => Member::Unnamed(Index { index: index as u32, span: field.ty.span() }),
        };
        match classify(field, index) {
            Ok(FieldRole::Site(spec)) => {
                site_specs.push(spec);
                site_members.push(member);
            }
            Ok(FieldRole::Default) => default_members.push(member),
            Err(e) => errors.push(e),
        }
    }
    if let Some(e) = combine(errors) {
        return Err(e);
    }

    let scope = args.scope.as_ref().map_or_else(|| quote!(#ulo::scope::Auto), |s| s.path());
    let bindings = sites::bindings(site_specs.len());
    let reads = sites::read(&site_specs, &bindings);
    let construct = quote! {
        #reads
        ::core::result::Result::Ok(Self {
            #(#site_members: #bindings,)*
            #(#default_members: ::core::default::Default::default(),)*
        })
    };

    let name = &item.ident;
    let (_, ty_generics, _) = item.generics.split_for_impl();
    let self_ty: Type = syn::parse_quote!(#name #ty_generics);
    let construct_impl = ConstructImpl {
        self_ty: &self_ty,
        generics: &item.generics,
        sites: sites::declare(&site_specs, &scope),
        scope,
        timeout: args.timeout.as_ref(),
        construct,
        construct_span: name.span(),
    }
    .emit();

    Ok(quote! {
        #item
        #construct_impl
    })
}

/// What one field contributes: a site, or a default.
pub(crate) enum FieldRole {
    Site(SiteSpec),
    Default,
}

/// Reads and strips the field's `#[injectable(default)]`; any other argument there is a span
/// error on it.
pub(crate) fn classify(field: &mut Field, index: usize) -> syn::Result<FieldRole> {
    let markers = attrs::take_all(&mut field.attrs, "injectable");
    let mut default = false;
    for marker in &markers {
        let arg: syn::Ident = marker.parse_args().map_err(|_| marker_error(marker))?;
        if arg != "default" {
            return Err(marker_error(marker));
        }
        if default {
            return Err(syn::Error::new_spanned(marker, "`#[injectable(default)]` is written once"));
        }
        default = true;
    }
    if default {
        return Ok(FieldRole::Default);
    }
    let label = match &field.ident {
        Some(ident) => SiteLabel::Field(ident.clone()),
        None => SiteLabel::Index(index),
    };
    Ok(FieldRole::Site(SiteSpec { label, ty: field.ty.clone(), span: field.ty.span() }))
}

fn marker_error(marker: &syn::Attribute) -> syn::Error {
    syn::Error::new_spanned(
        marker,
        "a field takes `#[injectable(default)]` alone, which sets it from `Default`; every other field is a site",
    )
}
