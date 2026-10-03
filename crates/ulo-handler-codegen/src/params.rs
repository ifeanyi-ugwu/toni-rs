//! Parameter analysis: a handler's receiver and its parameters, each read from the call by type
//! (transports DESIGN §2.2, §2.3).
//!
//! No parameter is classified by its spelling. Every parameter type goes through
//! `ulo_transport::__private::Param<T, M>`, implemented once for every `FromCall<T>` type and once
//! for every `FromContainer` type, the marker `M` inferred, so an alias or a renamed import means
//! what its type means.

use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::{FnArg, GenericParam, Ident, Pat, Signature, Type};

/// How the handler takes its controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    /// `&self`.
    Ref,
    /// `self: Arc<Self>`, a stable arbitrary self type: a streaming reply captures the `Arc`
    /// rather than borrowing the controller, which `+ use<>` on its opaque type requires.
    Arc,
}

/// One parameter after the receiver.
#[derive(Clone)]
pub struct Param {
    /// From 0, among the parameters after the receiver.
    pub index: usize,
    /// As diagnostics name it: the binding's identifier, or the pattern as written for a
    /// destructuring pattern such as `Path(id)`.
    pub name: String,
    pub ty: Type,
    /// The parameter's type, where an assertion about it points.
    pub span: Span,
}

/// A handler's signature as the transport attribute reads it.
pub struct HandlerSig {
    pub ident: Ident,
    pub receiver: Receiver,
    pub params: Vec<Param>,
    pub is_async: bool,
    /// The handler declares a type or const parameter: its opaque return types keep their
    /// captures, and its parameter checks are read from the mount function rather than from a
    /// free constant.
    pub is_generic: bool,
}

/// Reads `sig`. A handler takes `&self` or `self: Arc<Self>`; `self`, `&mut self`, another typed
/// receiver or no receiver at all is a span error on the receiver or the name.
pub fn analyze(sig: &Signature) -> syn::Result<HandlerSig> {
    let mut inputs = sig.inputs.iter();
    let receiver = match inputs.next() {
        Some(FnArg::Receiver(receiver)) => receiver_kind(receiver)?,
        _ => {
            return Err(syn::Error::new(
                sig.ident.span(),
                "a handler is a method: it takes `&self`, or `self: Arc<Self>` for a reply that outlives the call",
            ));
        }
    };
    let params = inputs
        .enumerate()
        .map(|(index, input)| match input {
            FnArg::Typed(typed) => Ok(Param { index, name: param_name(&typed.pat), ty: (*typed.ty).clone(), span: typed.ty.span() }),
            FnArg::Receiver(receiver) => Err(syn::Error::new_spanned(receiver, "a second receiver")),
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let is_generic = sig.generics.params.iter().any(|p| matches!(p, GenericParam::Type(_) | GenericParam::Const(_)));
    Ok(HandlerSig { ident: sig.ident.clone(), receiver, params, is_async: sig.asyncness.is_some(), is_generic })
}

fn receiver_kind(receiver: &syn::Receiver) -> syn::Result<Receiver> {
    let _ = receiver;
    todo!("`&self` → Ref; `self: Arc<Self>` (any path ending in `Arc` with argument `Self`) → Arc; anything else a span error naming the two forms")
}

fn param_name(pat: &Pat) -> String {
    match pat {
        Pat::Ident(ident) => syn::ext::IdentExt::unraw(&ident.ident).to_string(),
        other => other.to_token_stream().to_string(),
    }
}
