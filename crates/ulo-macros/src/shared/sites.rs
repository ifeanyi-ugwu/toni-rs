//! Sites as the macros see them: a field or parameter with its type, emitted twice: its
//! declaration in `Construct::sites`, which carries the `quote_spanned!` assertion that points a
//! site error at the site itself, and its read in `Construct::construct`.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::{Ident, Type};

use crate::shared::{resolver_param, sites_param, ulo};

pub(crate) struct SiteSpec {
    pub(crate) label: SiteLabel,
    pub(crate) ty: Type,
    /// The span of the site's type, where its assertion's error points.
    pub(crate) span: Span,
}

pub(crate) enum SiteLabel {
    Field(Ident),
    /// A tuple struct's field, declared under its index, so the wiring report reads
    /// ``field `2` `` even when a defaulted field sits between two sites.
    Index(usize),
    Param(Ident),
}

/// `::ulo::__private::field::<Ty, #scope>(s, "name");` or `param::<..>` for each site, spanned at
/// the site's type: the call declares the site and asserts `Site + AllowedIn<#scope>` in one.
pub(crate) fn declare(sites: &[SiteSpec], scope: &TokenStream) -> TokenStream {
    let ulo = ulo();
    let s = sites_param();
    let calls = sites.iter().map(|site| {
        let ty = &site.ty;
        let (helper, name) = match &site.label {
            SiteLabel::Field(ident) => (quote!(field), label_text(ident)),
            SiteLabel::Index(index) => (quote!(field), index.to_string()),
            SiteLabel::Param(ident) => (quote!(param), label_text(ident)),
        };
        quote_spanned! {site.span=>
            #ulo::__private::#helper::<#ty, #scope>(#s, #name);
        }
    });
    quote!(#(#calls)*)
}

/// `let #binding = <#ty as ::ulo::Site>::read(r).await?;` for each site, in order. A failed read
/// propagates as `ConstructError::Site` through `From<LookupError>`.
pub(crate) fn read(sites: &[SiteSpec], bindings: &[Ident]) -> TokenStream {
    let ulo = ulo();
    let r = resolver_param();
    let reads = sites.iter().zip(bindings).map(|(site, binding)| {
        let ty = &site.ty;
        quote_spanned! {site.span=>
            let #binding = <#ty as #ulo::Site>::read(#r).await?;
        }
    });
    quote!(#(#reads)*)
}

/// One binding per site, mixed-site so no name the user writes can collide with it.
pub(crate) fn bindings(count: usize) -> Vec<Ident> {
    (0..count).map(|i| Ident::new(&format!("__ulo_site_{i}"), Span::mixed_site())).collect()
}

fn label_text(ident: &Ident) -> String {
    syn::ext::IdentExt::unraw(ident).to_string()
}
