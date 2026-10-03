//! `#[routes]` on a controller's handler impl (§7, transports DESIGN §2.1, §2.5).
//!
//! It runs before the transports' handler attributes, which sit on the methods inside it, so it
//! hands each handler's enhancers and metadata to them rather than registering handlers itself:
//! 1. The impl's `#[guards]`, `#[interceptors]`, `#[error_handlers]` and `#[meta]` are the
//!    controller tier.
//! 2. A method is a handler when it carries an attribute outside `shared::attrs::is_inert`, read
//!    through `cfg_attr`; that attribute is its transport's. Its own enhancer and `#[meta]`
//!    attributes, inside `cfg_attr` or not, are its method tier.
//! 3. Each handler's enhancer and `#[meta]` attributes are removed and one `__handler` attribute
//!    (`ulo_handler_codegen::protocol`) is appended as its last attribute, so it sits after the
//!    transport attribute whatever else the method carries. The transport attribute consumes it
//!    and writes `__ULO_KEY_<name>`, `__ULO_CHECKS_<name>` and `__ulo_mount_<name>` into the impl.
//!    When the transport attribute sits inside `cfg_attr`, `__handler` goes inside a `cfg_attr`
//!    with the same predicates, and an enhancer or `#[meta]` attribute taken from inside one
//!    travels with its predicates as `#[cfg(..)]` gates.
//!    On a controller with type or const parameters, each handler's opaque return types first
//!    gain `+ use<T, ..>` naming them, which `use<..>` must and which the transport attribute,
//!    seeing the method alone, cannot learn; it then leaves those types as written.
//! 4. After the impl: one assertion per controller-level transport key against the handlers'
//!    `__ULO_KEY_*` constants, and one read of each handler's `__ULO_CHECKS_*`, as free constants;
//!    for a generic controller, named associated constants that `mount` reads instead (X1).
//! 5. `impl ::ulo::Controller for Self`, whose `mount` builds the impl-level `value` entries once
//!    into a `::ulo::__private::Shared` and calls every `__ulo_mount_<name>` with it, in method
//!    order (X2).
//!
//! A handler's `#[cfg]` and `#[cfg_attr]` reach `#[routes]` unevaluated, and rustc removes a
//! cfg'd-out method, or expands a `cfg_attr` around its transport attribute, only after `#[routes]`
//! has expanded. Each read of a handler's `__ULO_KEY_*` or `__ULO_CHECKS_*` in step 4, and its
//! mount call in step 5, carries the handler's gates: its `cfg` gates
//! (`ulo_handler_codegen::cfg::presence_gates`) and the predicates of the `cfg_attr` around its
//! transport attribute.
//!
//! A controller-level enhancer applies to every handler, strictly; the transport-scoped form
//! `http = AuthGuard` applies to that transport's handlers alone.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::Parser;
use syn::{Attribute, GenericParam, ImplItem, ImplItemFn, ItemImpl};
use ulo_handler_codegen::protocol::{self, EnhancerAttr, HandlerTokens, MetaExpr, MetaTokens, Role};
use ulo_handler_codegen::{cfg, keys, reply, shared};

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

    let captures_impl_params =
        item.generics.params.iter().any(|param| matches!(param, GenericParam::Type(_) | GenericParam::Const(_)));
    let mut handlers = Vec::new();
    let mut errors = Vec::new();
    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else { continue };
        match rewrite_handler(method, &controller, &controller_meta) {
            Ok(Some(handler)) => {
                if captures_impl_params {
                    reply::rewrite_opaque_returns_in(&mut method.sig, &item.generics);
                }
                handlers.push(handler);
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if let Some(e) = combine(errors) {
        return Err(e);
    }

    let gated: Vec<keys::GatedHandler<'_>> =
        handlers.iter().map(|handler| keys::GatedHandler { name: &handler.name, gates: &handler.gates }).collect();
    let scoped = keys::scoped_keys(&controller);
    let assertions = keys::assertions(&item.self_ty, &item.generics, &scoped, &gated);
    if !assertions.associated.is_empty() {
        item.items.push(ImplItem::Verbatim(assertions.associated.clone()));
    }

    // `_m` when every mount call sits behind a `cfg`, or there is none, so a build compiling them
    // all out raises no unused-parameter warning.
    let always_mounts = handlers.iter().any(|handler| handler.gates.is_empty());
    let mount = syn::Ident::new(if always_mounts { "m" } else { "_m" }, Span::call_site());
    let shared = shared::shared_ident();
    let build_shared = (!handlers.is_empty()).then(|| {
        let construct = shared::construct(&shared::shared_values(&controller));
        quote!(let #shared = #construct;)
    });
    let calls = handlers.iter().map(|handler| {
        let mount_fn = format_ident!("__ulo_mount_{}", handler.name.unraw());
        let gates = &handler.gates;
        quote_spanned! {handler.name.span()=>
            #(#gates)*
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
    /// The method's attributes that decide whether it is compiled.
    pub(crate) gates: Vec<Attribute>,
}

/// Classifies `method`: `Some` for a handler, with its method tier removed and the `__handler`
/// attribute appended after its transport attribute; `None` for any other item.
///
/// The method's attributes are read through `cfg_attr` ([`cfg::leaves`]). Its transport attribute
/// is an attribute outside `attrs::is_inert`: with one written outside any `cfg_attr`, the method
/// is a handler in every build; otherwise the first one inside a `cfg_attr` decides, and the method is a handler where that attribute's predicates hold. The
/// `__handler` attribute is then appended inside a `cfg_attr` with the same predicates, and those
/// predicates join the handler's gates. Each enhancer and `#[meta]` attribute inside a `cfg_attr`
/// is taken with its predicates as gates.
pub(crate) fn rewrite_handler(
    method: &mut ImplItemFn,
    controller: &[EnhancerAttr],
    controller_meta: &[MetaExpr],
) -> syn::Result<Option<Handler>> {
    let leaves = cfg::leaves(&method.attrs);
    let transport = if leaves.iter().any(|leaf| leaf.predicates.is_empty() && !attrs::is_inert(&leaf.attr)) {
        Some(Vec::new())
    } else {
        leaves.iter().find(|leaf| !attrs::is_inert(&leaf.attr)).map(|leaf| leaf.predicates.clone())
    };
    let (method_tier, method_meta) = take_method_tier(&mut method.attrs)?;
    let Some(transport_predicates) = transport else {
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
                &first.expr,
                format!("#[meta] on `{}`, which carries no transport attribute and so is not a handler", method.sig.ident.unraw()),
            ));
        }
        return Ok(None);
    };

    let mut gates = cfg::presence_gates(&method.attrs);
    gates.extend(cfg::gates_of(&transport_predicates));
    let tokens = HandlerTokens {
        handler: method.sig.ident.clone(),
        controller: controller.to_vec(),
        method: method_tier,
        meta: MetaTokens { controller: controller_meta.to_vec(), method: method_meta },
    };
    let appended = Attribute::parse_outer.parse2(cfg::under(&transport_predicates, tokens.to_meta()))?;
    method.attrs.extend(appended);
    Ok(Some(Handler { name: tokens.handler, gates }))
}

/// Removes a method's enhancer and `#[meta]` attributes, those inside `cfg_attr` included, and
/// parses them in the order written, each with its `cfg_attr` predicates as gates.
fn take_method_tier(attrs: &mut Vec<Attribute>) -> syn::Result<(Vec<EnhancerAttr>, Vec<MetaExpr>)> {
    let taken = cfg::take(attrs, |attr| role_of(attr).is_some() || protocol::is_meta(attr));
    let mut enhancers = Vec::new();
    let mut meta = Vec::new();
    let mut errors = Vec::new();
    for leaf in taken {
        let gates = leaf.gates();
        let parsed = match role_of(&leaf.attr) {
            Some(role) => EnhancerAttr::from_attr(role, &leaf.attr).map(|attr| enhancers.push(EnhancerAttr { gates, ..attr })),
            None => protocol::meta_exprs(&leaf.attr, &gates).map(|exprs| meta.extend(exprs)),
        };
        if let Err(e) = parsed {
            errors.push(e);
        }
    }
    match combine(errors) {
        Some(e) => Err(e),
        None => Ok((enhancers, meta)),
    }
}

fn role_of(attr: &Attribute) -> Option<Role> {
    attr.path().segments.last().and_then(|s| Role::of_attr(&s.ident.to_string()))
}

/// Removes every enhancer attribute from `attrs` and parses it, in the order written.
fn take_enhancers(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<EnhancerAttr>> {
    let mut taken = Vec::new();
    let mut errors = Vec::new();
    attrs.retain(|attr| {
        let Some(role) = role_of(attr) else {
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
