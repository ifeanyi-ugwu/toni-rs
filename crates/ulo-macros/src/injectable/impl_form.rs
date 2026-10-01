//! The impl form: the constructor is the fn named `new`, or the one marked `#[construct]`. Two
//! marked fns, or neither a marked fn nor `new`, is a span error. Its parameters are sites; it
//! may be `async`; its return is `Self` or `Result<Self, E>`, told apart by type through
//! `::ulo::__private::IntoConstructed` rather than by the spelling of the return type, so an
//! alias for a `Result` reads as one. A constructor whose future is not `Send` fails at the
//! generated `Construct` impl, pointing at the function.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, ImplItem, ImplItemFn, ItemImpl, Pat, ReturnType};

use crate::injectable::InjectableArgs;
use crate::shared::attrs;
use crate::shared::sites::{self, SiteLabel, SiteSpec};
use crate::shared::{ConstructImpl, ulo};

/// The impl as written, minus `#[construct]`, followed by the `Construct` impl for its self type.
pub(crate) fn expand(args: InjectableArgs, mut item: ItemImpl) -> syn::Result<TokenStream> {
    if let Some((_, path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "#[injectable] goes on the type's inherent impl block holding its constructor, not on a trait impl",
        ));
    }
    let ulo = ulo();
    let ctor = find_constructor(&mut item)?;
    let site_specs = params(ctor)?;
    let name = &ctor.sig.ident;
    let await_ctor = ctor.sig.asyncness.map(|_| quote!(.await));
    let construct_span = name.span();

    let scope = args.scope.as_ref().map_or_else(|| quote!(#ulo::scope::Auto), |s| s.path());
    let bindings = sites::bindings(site_specs.len());
    let reads = sites::read(&site_specs, &bindings);
    let returned_span = match &ctor.sig.output {
        ReturnType::Type(_, ty) => ty.span(),
        ReturnType::Default => construct_span,
    };
    let returned = quote_spanned! {returned_span=>
        #ulo::__private::IntoConstructed::<Self>::into_constructed(Self::#name(#(#bindings),*) #await_ctor)
    };
    let construct = quote! {
        #reads
        #returned
    };
    let sites_body = sites::declare(&site_specs, &scope);

    let construct_impl = ConstructImpl {
        self_ty: &item.self_ty,
        generics: &item.generics,
        scope,
        timeout: args.timeout.as_ref(),
        sites: sites_body,
        construct,
        construct_span,
    }
    .emit();

    Ok(quote! {
        #item
        #construct_impl
    })
}

/// The constructor: the one fn marked `#[construct]` (stripped), else the fn named `new`.
pub(crate) fn find_constructor(item: &mut ItemImpl) -> syn::Result<&ImplItemFn> {
    let mut marked: Option<usize> = None;
    for (index, impl_item) in item.items.iter_mut().enumerate() {
        let ImplItem::Fn(method) = impl_item else { continue };
        let markers = attrs::take_all(&mut method.attrs, "construct");
        for marker in &markers {
            if !matches!(marker.meta, syn::Meta::Path(_)) {
                return Err(syn::Error::new_spanned(marker, "#[construct] takes no arguments"));
            }
        }
        if let Some(marker) = markers.get(1) {
            return Err(syn::Error::new_spanned(marker, "#[construct] is written once on a fn"));
        }
        if let Some(marker) = markers.first() {
            if marked.is_some() {
                return Err(syn::Error::new_spanned(
                    marker,
                    "a second #[construct] fn; a type has one constructor, so mark one fn only",
                ));
            }
            marked = Some(index);
        }
    }

    let found = item.items.iter().enumerate().find_map(|(index, impl_item)| match impl_item {
        ImplItem::Fn(method) if marked.map_or(method.sig.ident == "new", |m| m == index) => Some(method),
        _ => None,
    });
    found.ok_or_else(|| {
        syn::Error::new_spanned(
            &item.self_ty,
            "no constructor: name it `new`, or mark the fn that builds the type #[construct]",
        )
    })
}

/// The constructor's parameters as sites. A receiver, or a pattern other than an identifier,
/// is a span error on it.
pub(crate) fn params(ctor: &ImplItemFn) -> syn::Result<Vec<SiteSpec>> {
    let mut specs = Vec::new();
    for input in &ctor.sig.inputs {
        match input {
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "a constructor takes no `self`: it builds the instance from its parameters, each an injection site",
                ));
            }
            FnArg::Typed(typed) => match &*typed.pat {
                Pat::Ident(pat) if pat.by_ref.is_none() && pat.subpat.is_none() => specs.push(SiteSpec {
                    label: SiteLabel::Param(pat.ident.clone()),
                    ty: (*typed.ty).clone(),
                    span: typed.ty.span(),
                }),
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "a constructor parameter is named by a plain identifier, which diagnostics print as the site's name",
                    ));
                }
            },
        }
    }
    Ok(specs)
}
