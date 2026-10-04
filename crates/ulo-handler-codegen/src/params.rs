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
use syn::{FnArg, GenericArgument, GenericParam, Ident, Pat, PathArguments, Signature, Type};

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
    /// The binding's identifier, unraw'd, as a wiring report names the parameter; `None` for a
    /// destructuring pattern, which the report names by `index`.
    pub ident: Option<String>,
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
    /// The handler declares a type or const parameter. Its opaque return types keep their
    /// captures, and [`crate::emit::MountFn::emit`] refuses it: the generated call names each
    /// parameter's type outside the method, where the method's own parameters are not in scope.
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
            FnArg::Typed(typed) => Ok(Param {
                index,
                name: param_name(&typed.pat),
                ident: param_ident(&typed.pat),
                ty: (*typed.ty).clone(),
                span: typed.ty.span(),
            }),
            FnArg::Receiver(receiver) => Err(syn::Error::new_spanned(receiver, "a second receiver")),
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let is_generic = sig.generics.params.iter().any(|p| matches!(p, GenericParam::Type(_) | GenericParam::Const(_)));
    Ok(HandlerSig { ident: sig.ident.clone(), receiver, params, is_async: sig.asyncness.is_some(), is_generic })
}

/// `&self`, also written `self: &Self`, or `self: Arc<Self>` with any path ending in `Arc`.
/// `syn` gives `&self` the type `&Self`, so one match covers both spellings of the first.
fn receiver_kind(receiver: &syn::Receiver) -> syn::Result<Receiver> {
    match &*receiver.ty {
        Type::Reference(reference) if reference.mutability.is_none() && is_self(&reference.elem) => Ok(Receiver::Ref),
        ty if is_arc_of_self(ty) => Ok(Receiver::Arc),
        _ => Err(syn::Error::new_spanned(
            receiver,
            "a handler takes `&self`, or `self: Arc<Self>` for a reply that outlives the call",
        )),
    }
}

fn is_self(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.qself.is_none() && path.path.is_ident("Self"))
}

fn is_arc_of_self(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    let Some(last) = path.path.segments.last() else { return false };
    if path.qself.is_some() || last.ident != "Arc" {
        return false;
    }
    let PathArguments::AngleBracketed(arguments) = &last.arguments else { return false };
    let mut arguments = arguments.args.iter();
    matches!((arguments.next(), arguments.next()), (Some(GenericArgument::Type(inner)), None) if is_self(inner))
}

fn param_name(pat: &Pat) -> String {
    match param_ident(pat) {
        Some(ident) => ident,
        None => pat.to_token_stream().to_string(),
    }
}

/// `mut` and a `@` subpattern dropped: `mut r#type` and `r#type @ Path(..)` both give `type`.
fn param_ident(pat: &Pat) -> Option<String> {
    match pat {
        Pat::Ident(ident) => Some(syn::ext::IdentExt::unraw(&ident.ident).to_string()),
        _ => None,
    }
}
