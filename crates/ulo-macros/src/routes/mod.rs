//! `#[routes]` on a controller's handler impl (§7, transports DESIGN §2.1, §2.5).
//!
//! It runs before the transports' handler attributes, which sit on the methods inside it, so it
//! hands each handler's enhancers and metadata to them rather than registering handlers itself:
//! 1. The impl's `#[guards]`, `#[interceptors]`, `#[error_handlers]` and `#[meta]` are the
//!    controller tier.
//! 2. A method is a handler when it carries an attribute outside `shared::attrs::is_inert`; that
//!    attribute is its transport's. Its own enhancer and `#[meta]` attributes are its method tier.
//! 3. Each handler's enhancer and `#[meta]` attributes are removed and one `__handler` attribute
//!    (`ulo_handler_codegen::protocol`) is appended as its last attribute, so it sits after the
//!    transport attribute whatever else the method carries. The transport attribute consumes it
//!    and writes `__ULO_KEY_<name>`, `__ULO_CHECKS_<name>` and `__ulo_mount_<name>` into the impl.
//! 4. After the impl: one assertion per controller-level transport key against the handlers'
//!    `__ULO_KEY_*` constants, and one read of each handler's `__ULO_CHECKS_*`, as free constants;
//!    for a generic controller, named associated constants that `mount` reads instead (X1).
//! 5. `impl ::ulo::Controller for Self`, whose `mount` builds the impl-level `value` entries once
//!    into a `::ulo::__private::Shared` and calls every `__ulo_mount_<name>` with it, in method
//!    order (X2).
//!
//! A controller-level enhancer applies to every handler, strictly; the transport-scoped form
//! `http = AuthGuard` applies to that transport's handlers alone.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::Parser;
use syn::{Attribute, Expr, ImplItem, ImplItemFn, ItemImpl};
use ulo_handler_codegen::protocol::{self, EnhancerAttr, HandlerTokens, MetaTokens, Role};
use ulo_handler_codegen::{keys, shared};

use crate::shared::{attrs, combine, ulo};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(attr, "#[routes] takes no arguments"));
    }
    let item: ItemImpl = syn::parse2(item)?;
    expand_impl(item)
}

/// The impl with each handler's attributes rewritten and the controller-tier assertions added,
/// followed by `impl Controller` and the free assertions.
pub(crate) fn expand_impl(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if let Some((_, path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "#[routes] goes on the controller's inherent impl block holding its handlers, not on a trait impl",
        ));
    }
    let ulo = ulo();
    let controller = take_enhancers(&mut item.attrs)?;
    let controller_meta = protocol::take_meta(&mut item.attrs)?;

    let mut handlers = Vec::new();
    let mut errors = Vec::new();
    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else { continue };
        match rewrite_handler(method, &controller, &controller_meta) {
            Ok(Some(handler)) => handlers.push(handler),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if let Some(e) = combine(errors) {
        return Err(e);
    }

    let names: Vec<syn::Ident> = handlers.iter().map(|handler| handler.name.clone()).collect();
    let scoped = keys::scoped_keys(&controller);
    let assertions = keys::assertions(&item.self_ty, &item.generics, &scoped, &names);
    if !assertions.associated.is_empty() {
        item.items.push(ImplItem::Verbatim(assertions.associated.clone()));
    }

    let mount = syn::Ident::new(if handlers.is_empty() { "_m" } else { "m" }, Span::call_site());
    let shared = shared::shared_ident();
    let build_shared = (!handlers.is_empty()).then(|| {
        let construct = shared::construct(&shared::shared_values(&controller));
        quote!(let #shared = #construct;)
    });
    let calls = handlers.iter().map(|handler| {
        let mount_fn = format_ident!("__ulo_mount_{}", handler.name.unraw());
        quote_spanned! {handler.name.span()=>
            Self::#mount_fn(#mount, &#shared);
        }
    });
    let in_mount = &assertions.in_mount;
    let self_ty = &item.self_ty;
    let (impl_generics, _, where_clause) = item.generics.split_for_impl();
    let controller_impl = quote! {
        impl #impl_generics #ulo::Controller for #self_ty #where_clause {
            fn mount(#mount: &mut #ulo::Mount<'_>) {
                #in_mount
                #build_shared
                #(#calls)*
            }
        }
    };
    let free = &assertions.free;

    Ok(quote! {
        #item
        #controller_impl
        #free
    })
}

/// A handler method; its enhancer attributes and metadata travel in the `__handler` attribute.
pub(crate) struct Handler {
    pub(crate) name: syn::Ident,
}

/// Classifies `method`: `Some` for a handler, with its method tier removed and the `__handler`
/// attribute appended after its transport attribute; `None` for any other item.
pub(crate) fn rewrite_handler(
    method: &mut ImplItemFn,
    controller: &[EnhancerAttr],
    controller_meta: &[Expr],
) -> syn::Result<Option<Handler>> {
    let is_handler = method.attrs.iter().any(|attr| !attrs::is_inert(attr));
    let method_tier = take_enhancers(&mut method.attrs)?;
    let method_meta = protocol::take_meta(&mut method.attrs)?;
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
        if let Some(first) = method_meta.first() {
            return Err(syn::Error::new_spanned(
                first,
                format!("#[meta] on `{}`, which carries no transport attribute and so is not a handler", method.sig.ident.unraw()),
            ));
        }
        return Ok(None);
    }

    let tokens = HandlerTokens {
        handler: method.sig.ident.clone(),
        controller: controller.to_vec(),
        method: method_tier,
        meta: MetaTokens { controller: controller_meta.to_vec(), method: method_meta },
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
