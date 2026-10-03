//! Transport keys (transports DESIGN §2.1, X1): the constant each transport attribute emits per
//! handler, and the assertions `#[routes]` emits over every controller-level scoped key.
//!
//! A free `const _` after the impl is evaluated unconditionally; inside the impl a constant needs
//! a name, and a named associated constant is evaluated only where it is read, so a misspelled
//! key would compile unnoticed. Const evaluation runs after every attribute has expanded, so the
//! `__ULO_KEY_*` constants exist by then. A generic controller is the case a free constant cannot
//! name: there `#[routes]` emits a named associated constant and `Controller::mount` reads it,
//! which evaluates it when `mount` is instantiated.

use proc_macro2::{Span, TokenStream};
use syn::{Generics, Ident, Type};

use crate::paths::Paths;
use crate::protocol::EnhancerAttr;

/// `__ULO_KEY_<name>`.
pub fn key_const_ident(name: &str, span: Span) -> Ident {
    Ident::new(&format!("__ULO_KEY_{name}"), span)
}

/// `__ULO_CHECKS_<name>`.
pub fn checks_const_ident(name: &str, span: Span) -> Ident {
    Ident::new(&format!("__ULO_CHECKS_{name}"), span)
}

/// `const __ULO_KEY_<name>: &'static str = <Marker as ::ulo::Transport>::KEY;`, hidden, with
/// `non_upper_case_globals` allowed for the method name inside it.
pub fn key_const(name: &str, span: Span, paths: &Paths) -> TokenStream {
    let _ = (name, span, paths);
    todo!("the associated constant above")
}

/// Every distinct transport key written at the controller tier, in the order first written, each
/// with the span of its first occurrence.
pub fn scoped_keys(controller: &[EnhancerAttr]) -> Vec<Ident> {
    let _ = controller;
    todo!("the `transport` of every entry, deduplicated by `unraw` text")
}

/// What `#[routes]` emits for the X1 key assertions and for the per-handler checks constants.
pub struct Assertions {
    /// Free items after the impl: `const _: () = assert!(::ulo::__private::key_in("htpp", &[Ctrl::__ULO_KEY_get, ..]), "..");`
    /// per scoped key, spanned on the key, and `const _: () = <Ctrl>::__ULO_CHECKS_<name>;` per
    /// handler. Empty for a generic controller.
    pub free: TokenStream,
    /// Associated items `#[routes]` adds to the impl: for a generic controller, the named
    /// `const __ULO_KEYS_CHECK_<key>: () = assert!(..);` per scoped key. Empty otherwise.
    pub associated: TokenStream,
    /// Statements at the head of `Controller::mount`: for a generic controller,
    /// `let () = Self::__ULO_KEYS_CHECK_<key>;` and `let () = Self::__ULO_CHECKS_<name>;`.
    /// Empty otherwise.
    pub in_mount: TokenStream,
}

/// The assertions for `keys` over `handlers`, the handler identifiers of the impl. The message
/// names the key and every handler: "`htpp` is not the key of any handler's transport in this
/// impl (handlers: get, get_rpc)".
pub fn assertions(self_ty: &Type, generics: &Generics, keys: &[Ident], handlers: &[Ident]) -> Assertions {
    let _ = (self_ty, generics, keys, handlers);
    todo!("free consts for a non-generic controller; associated consts and mount reads for a generic one")
}
