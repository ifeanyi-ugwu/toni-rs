//! `#[routes]` on a controller's handler impl (§7).
//!
//! It runs before the transports' handler attributes, which sit on the methods inside it, so it
//! hands each handler's enhancers to them rather than registering handlers itself:
//! 1. The impl's `#[guards]`, `#[interceptors]` and `#[error_handlers]` are the controller tier.
//! 2. A method is a handler when it carries an attribute outside `shared::attrs::is_inert`; that
//!    attribute is its transport's. Its own enhancer attributes are its method tier.
//! 3. Each handler's enhancer attributes are removed and one
//!    `#[::ulo::__private::__handler(controller(..), method(..))]` is appended after its
//!    transport attribute. The transport attribute consumes it and writes
//!    `fn __ulo_mount_<name>(m: &mut ::ulo::Mount<'_>)` into the impl, building both tiers with
//!    `::ulo::__private::__enhancer_specs!(<Transport>, "<key>", <tokens>)`.
//! 4. `impl ::ulo::Controller for Self` calls every `__ulo_mount_<name>` in method order.
//!
//! A controller-level enhancer applies to every handler, strictly; the transport-scoped form
//! `http = AuthGuard` applies to that transport's handlers alone.

use proc_macro2::TokenStream;
use syn::{ImplItemFn, ItemImpl};

use crate::enhancers::EnhancerAttr;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(attr, "#[routes] takes no arguments"));
    }
    let item: ItemImpl = syn::parse2(item)?;
    expand_impl(item)
}

/// The impl with each handler's attributes rewritten, followed by `impl Controller`.
pub(crate) fn expand_impl(mut item: ItemImpl) -> syn::Result<TokenStream> {
    todo!()
}

/// The impl-level enhancer attributes, removed from the impl.
pub(crate) fn controller_tier(item: &mut ItemImpl) -> syn::Result<Vec<EnhancerAttr>> {
    todo!()
}

/// A handler method and the enhancer attributes taken off it.
pub(crate) struct Handler {
    pub(crate) name: syn::Ident,
    pub(crate) method_tier: Vec<EnhancerAttr>,
}

/// Classifies `method`: `Some` for a handler, with its method tier removed and the
/// `__handler` attribute appended after its transport attribute; `None` for any other item.
pub(crate) fn rewrite_handler(method: &mut ImplItemFn, controller: &[EnhancerAttr]) -> syn::Result<Option<Handler>> {
    todo!()
}
