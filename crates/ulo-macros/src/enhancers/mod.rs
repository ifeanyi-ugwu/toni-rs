//! The enhancer attributes and the hidden protocol that carries them to a transport (§7).
//!
//! Grammar of `#[guards(..)]`, `#[interceptors(..)]` and `#[error_handlers(..)]`, a comma list of:
//! - `Type`: by type, resolved from the container; `spec.guard::<Type>()`.
//! - `key = Type`: by type, applied to handlers of the transport whose scope key is `key`
//!   (`http`, `rpc`, `grpc`, `ws`, as each transport crate declares).
//! - `value = expr`: by value, built once and shared; `spec.guard_value(expr)`.
//! - `with = |site: Ty, ..| expr`: by closure, built per execution. The closure's body is
//!   wrapped as `async move { body }` and handed to `spec.guard_with(..)`, whose parameters are
//!   sites.
//! - `key(value = expr)` and `key(with = ..)`: the last two, transport-scoped.
//!
//! Every expansion asserts the role for the handler's transport through
//! `::ulo::__private::assert_guard::<Type, Transport>()` inside a fn named after the handler,
//! so a missing role reads "`AuthGuard` is not a guard for `Rpc`" and names the handler.

use proc_macro2::{Span, TokenStream};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprClosure, Ident, Type};

/// Which role an attribute declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Guard,
    Interceptor,
    ErrorHandler,
}

impl Role {
    pub(crate) fn of_attr(name: &str) -> Option<Role> {
        match name {
            "guards" => Some(Role::Guard),
            "interceptors" => Some(Role::Interceptor),
            "error_handlers" => Some(Role::ErrorHandler),
            _ => None,
        }
    }
}

/// One attribute's entries, with its role.
pub(crate) struct EnhancerAttr {
    pub(crate) role: Role,
    pub(crate) entries: Vec<Entry>,
    pub(crate) span: Span,
}

pub(crate) struct Entry {
    /// The transport scope key, for the transport-scoped forms.
    pub(crate) scope: Option<Ident>,
    pub(crate) form: Form,
}

pub(crate) enum Form {
    Type(Type),
    Value(Expr),
    With(ExprClosure),
}

impl Parse for Entry {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        todo!()
    }
}

impl EnhancerAttr {
    pub(crate) fn from_attr(role: Role, attr: &syn::Attribute) -> syn::Result<Self> {
        todo!()
    }
}

/// `#[guards]` and its kin reached as attribute macros: outside a `#[routes]` impl, where
/// nothing consumed them.
pub(crate) fn marker(name: &str, _attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(syn::Error::new(
        Span::call_site(),
        format!("#[{name}] goes on a #[routes] impl block or on one of its handler methods"),
    ))
}

/// `__handler` reached as an attribute macro: the method has no transport attribute that
/// consumed it.
pub(crate) fn unconsumed_handler(_attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(syn::Error::new(
        Span::call_site(),
        "this handler's transport attribute did not read its enhancers; the transport crate may not support #[routes]",
    ))
}

/// The tokens `__handler(..)` carries for one handler.
pub(crate) struct HandlerTokens {
    pub(crate) handler: Ident,
    pub(crate) controller: Vec<EnhancerAttr>,
    pub(crate) method: Vec<EnhancerAttr>,
}

impl HandlerTokens {
    /// The attribute `#[routes]` appends: `#[::ulo::__private::__handler(name, controller(..), method(..))]`.
    pub(crate) fn to_attribute(&self) -> TokenStream {
        todo!()
    }
}

/// `__enhancer_specs!(<Transport type>, "<scope key>", <__handler tokens>)`: an expression
/// evaluating to `(EnhancerSpec<Transport>, EnhancerSpec<Transport>)`, controller tier then
/// method tier, keeping only the entries unscoped or scoped to this key, with the role
/// assertions.
pub(crate) fn specs(input: TokenStream) -> syn::Result<TokenStream> {
    todo!()
}
