//! Impl-level shared values (transports DESIGN §2.1, X2).
//!
//! A controller-level `value = expr` is built once, in the `Controller::mount` `#[routes]`
//! generates, into an `Arc`, and every handler's mount function receives the same
//! `::ulo::__private::Shared((Arc<V0>, Arc<V1>, ..))`. `#[routes]` cannot name a value's type, so
//! each `Vi` is a type parameter of the mount function, inferred at its call. The position of a
//! `value` entry, in the order written across the impl's three enhancer attributes and counting
//! entries scoped to other transports, is part of the `__handler` contract: [`shared_values`]
//! computes it the same way in `#[routes]` and in every transport attribute.

use proc_macro2::TokenStream;
use syn::{Expr, Ident};

use crate::paths::Paths;
use crate::protocol::{EnhancerAttr, Role};

/// One controller-level `value` entry.
#[derive(Clone)]
pub struct SharedValue {
    /// The position in `Shared`'s tuple.
    pub index: usize,
    pub role: Role,
    /// The transport key the entry is scoped to; `None` for every handler.
    pub transport: Option<Ident>,
    pub expr: Expr,
}

/// The controller tier's `value` entries, in order.
pub fn shared_values(controller: &[EnhancerAttr]) -> Vec<SharedValue> {
    let _ = controller;
    todo!("every `Form::Value` entry across the attrs in order, indexed from 0")
}

/// `__UloV<index>`, the mount function's type parameter for value `index`.
pub fn type_param(index: usize) -> Ident {
    Ident::new(&format!("__UloV{index}"), proc_macro2::Span::mixed_site())
}

/// `__ulo_shared`, the mount function's parameter and `Controller::mount`'s local.
pub fn shared_ident() -> Ident {
    Ident::new("__ulo_shared", proc_macro2::Span::mixed_site())
}

/// What `Controller::mount` evaluates once: `::ulo::__private::Shared((Arc::new(e0), ..,))`, or
/// `Shared(())` with no value.
pub fn construct(values: &[SharedValue]) -> TokenStream {
    let _ = values;
    todo!("the tuple of `::ulo::__private::Arc::new(expr)`, each spanned at its expression, trailing comma for one")
}

/// The mount function's generic parameters and the type of its `shared` parameter for a handler
/// of the transport `key`: `<__UloV0: ::ulo::Guard<Marker>, __UloV1>` and
/// `&::ulo::__private::Shared<(Arc<__UloV0>, Arc<__UloV1>,)>`. A value that applies to this
/// transport is bounded by its role, spanned at the handler's name, so a value lacking the role
/// fails E0277 at `Controller::mount`'s call of this handler's mount function.
pub fn mount_generics(values: &[SharedValue], key: &str, handler: &Ident, paths: &Paths) -> (TokenStream, TokenStream) {
    let _ = (values, key, handler, paths);
    todo!("bounded parameters for values that apply to `key`, unbounded for the rest")
}
