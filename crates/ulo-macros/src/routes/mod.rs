//! `#[routes]` on a controller's handler impl (§7).
//!
//! It runs before the transports' handler attributes, which sit on the methods inside it, so it
//! hands each handler's enhancers to them rather than registering handlers itself:
//! 1. The impl's `#[guards]`, `#[interceptors]` and `#[error_handlers]` are the controller tier,
//!    which takes no `value` entry.
//! 2. A method is a handler when it carries an attribute outside `shared::attrs::is_inert`; that
//!    attribute is its transport's. Its own enhancer attributes are its method tier.
//! 3. Each handler's enhancer attributes are removed and one
//!    `#[::ulo::__private::__handler(name, controller(..), method(..))]` is appended as its last
//!    attribute, so it sits after the transport attribute whatever else the method carries. The
//!    transport attribute consumes it and writes `fn __ulo_mount_<name>(m: &mut ::ulo::Mount<'_>)`
//!    into the impl, `<name>` being the method's identifier without `r#`, building both tiers with
//!    `::ulo::__private::__enhancer_specs!(<Transport>, "<key>", <tokens>)`.
//! 4. `impl ::ulo::Controller for Self` calls every `__ulo_mount_<name>` in method order.
//!
//! A controller-level enhancer applies to every handler, strictly; the transport-scoped form
//! `http = AuthGuard` applies to that transport's handlers alone.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::Parser;
use syn::{Attribute, ImplItem, ImplItemFn, ItemImpl};

use crate::enhancers::{EnhancerAttr, HandlerTokens, Role, refuse_controller_values};
use crate::shared::{attrs, combine, ulo};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(attr, "#[routes] takes no arguments"));
    }
    let item: ItemImpl = syn::parse2(item)?;
    expand_impl(item)
}

/// The impl with each handler's attributes rewritten, followed by `impl Controller`.
pub(crate) fn expand_impl(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if let Some((_, path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "#[routes] goes on the controller's inherent impl block holding its handlers, not on a trait impl",
        ));
    }
    let ulo = ulo();
    let controller = controller_tier(&mut item)?;

    let mut handlers = Vec::new();
    let mut errors = Vec::new();
    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else { continue };
        match rewrite_handler(method, &controller) {
            Ok(Some(handler)) => handlers.push(handler),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if let Some(e) = combine(errors) {
        return Err(e);
    }

    let mount = syn::Ident::new(if handlers.is_empty() { "_m" } else { "m" }, Span::call_site());
    let calls = handlers.iter().map(|handler| {
        let mount_fn = format_ident!("__ulo_mount_{}", handler.name.unraw());
        quote_spanned! {handler.name.span()=>
            Self::#mount_fn(#mount);
        }
    });
    let self_ty = &item.self_ty;
    let (impl_generics, _, where_clause) = item.generics.split_for_impl();
    let controller_impl = quote! {
        impl #impl_generics #ulo::Controller for #self_ty #where_clause {
            fn mount(#mount: &mut #ulo::Mount<'_>) {
                #(#calls)*
            }
        }
    };

    Ok(quote! {
        #item
        #controller_impl
    })
}

/// The impl-level enhancer attributes, removed from the impl. A `value` entry among them is an
/// error (§7).
pub(crate) fn controller_tier(item: &mut ItemImpl) -> syn::Result<Vec<EnhancerAttr>> {
    let tier = take_enhancers(&mut item.attrs)?;
    refuse_controller_values(&tier)?;
    Ok(tier)
}

/// A handler method; its enhancer attributes travel in the `__handler` attribute.
pub(crate) struct Handler {
    pub(crate) name: syn::Ident,
}

/// Classifies `method`: `Some` for a handler, with its method tier removed and the
/// `__handler` attribute appended after its transport attribute; `None` for any other item.
pub(crate) fn rewrite_handler(method: &mut ImplItemFn, controller: &[EnhancerAttr]) -> syn::Result<Option<Handler>> {
    let is_handler = method.attrs.iter().any(|attr| !attrs::is_inert(attr));
    let method_tier = take_enhancers(&mut method.attrs)?;
    if !is_handler {
        if let Some(first) = method_tier.first() {
            return Err(syn::Error::new(
                first.span,
                format!(
                    "#[{}] on `{}`, which carries no transport attribute and so is not a handler",
                    first.role.attr_name(),
                    method.sig.ident.unraw(),
                ),
            ));
        }
        return Ok(None);
    }

    let tokens = HandlerTokens {
        handler: method.sig.ident.clone(),
        controller: controller.to_vec(),
        method: method_tier,
    };
    let appended = Attribute::parse_outer.parse2(tokens.to_attribute())?;
    method.attrs.extend(appended);
    Ok(Some(Handler { name: tokens.handler }))
}

/// Removes every enhancer attribute from `attrs` and parses it, in the order written.
fn take_enhancers(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<EnhancerAttr>> {
    let mut taken = Vec::new();
    let mut errors = Vec::new();
    attrs.retain(|attr| {
        let Some(role) = attr.path().segments.last().and_then(|s| Role::of_attr(&s.ident.to_string())) else {
            return true;
        };
        match EnhancerAttr::from_attr(role, attr) {
            Ok(parsed) => taken.push(parsed),
            Err(e) => errors.push(e),
        }
        false
    });
    match combine(errors) {
        Some(e) => Err(e),
        None => Ok(taken),
    }
}
