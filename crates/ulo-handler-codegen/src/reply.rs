//! The reply side of a handler: the probe that sends every error side to the error handlers
//! whatever the return type is spelled as, and the `+ use<>` rewrite on streaming returns
//! (transports DESIGN §2.3).

use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::visit_mut::{self, VisitMut};
use syn::{
    CapturedParam, GenericParam, Generics, Ident, Lifetime, ParenthesizedGenericArguments, PreciseCapture,
    ReturnType, Signature, Token, TraitBound, TypeBareFn, TypeImplTrait, TypeParamBound, TypeReference,
};

use crate::paths::Paths;

/// The expression turning the handler's output `out` into `Result<T::Reply, BoxError>`:
///
/// ```text
/// (&&&<transport>::__private::IntoReplyProbe::<Marker, _>::new(out)).into_reply(&cx)
/// ```
///
/// with `ViaCallError`, `ViaBoxError` and `ViaValue` imported anonymously. The transport is the
/// probe type's parameter, since method lookup checks an impl's where-clauses and never a
/// method's, and `V: IntoReply<T>` is one of them.
///
/// The probe's three arms, each one reference deeper than the priority reads, so the first that
/// applies wins: `Result<V, E: Into<CallError>>` on `&&IntoReplyProbe` (`Err` becomes
/// `BoxError::from(CallError::from(e))`), `Result<V, E: Into<BoxError>>` on `&IntoReplyProbe`
/// (`Err` boxed unchanged), any `V: IntoReply<T>` on `IntoReplyProbe`. The compiler resolves a type
/// alias before method lookup, so an alias of `Result` takes the arm the `Result` takes.
pub fn reply_call(out: &Ident, cx: &Ident, paths: &Paths) -> TokenStream {
    let transport = &paths.transport;
    let marker = &paths.marker;
    quote! {
        {
            #[allow(unused_imports)]
            use #transport::__private::{ViaBoxError as _, ViaCallError as _, ViaValue as _};
            (&&&#transport::__private::IntoReplyProbe::<#marker, _>::new(#out)).into_reply(&#cx)
        }
    }
}

/// Appends `+ use<>` to every opaque type in `sig`'s return position, inside an `async fn`'s
/// return type included, so a reply outlives the call: on edition 2024 an opaque return type
/// captures `&self`'s lifetime whatever the hidden type borrows.
///
/// Left as written: every opaque type of a handler with a type or const parameter, and an opaque
/// type that already names a `use<..>` bound or a lifetime other than `'static`, an elided
/// reference such as `Item = &str` included. A stream that borrows `self` then fails inside the
/// handler body; the idiom is `self: Arc<Self>` as the receiver.
pub fn rewrite_opaque_returns(sig: &mut Signature) {
    rewrite_opaque_returns_in(sig, &Generics::default());
}

/// [`rewrite_opaque_returns`] for a handler of a generic controller, whose impl's type and const
/// parameters are in scope of every opaque return type and so must be named in its `use<..>`:
/// appends `+ use<T, N, ..>` listing them. `#[routes]` calls it before the transport attribute
/// expands, which then finds a `use<..>` and leaves the type as written; the transport attribute
/// sees the method alone and cannot learn the impl's parameters.
pub fn rewrite_opaque_returns_in(sig: &mut Signature, impl_generics: &Generics) {
    if has_type_or_const_param(&sig.generics) {
        return;
    }
    let ReturnType::Type(_, ty) = &mut sig.output else { return };
    let mut params = Punctuated::<CapturedParam, Token![,]>::new();
    for param in &impl_generics.params {
        match param {
            GenericParam::Type(param) => params.push(CapturedParam::Ident(param.ident.clone())),
            GenericParam::Const(param) => params.push(CapturedParam::Ident(param.ident.clone())),
            GenericParam::Lifetime(_) => {}
        }
    }
    let capture = PreciseCapture {
        use_token: Default::default(),
        lt_token: Default::default(),
        params,
        gt_token: Default::default(),
    };
    AppendCapture { capture }.visit_type_mut(ty);
}

fn has_type_or_const_param(generics: &Generics) -> bool {
    generics.params.iter().any(|param| matches!(param, GenericParam::Type(_) | GenericParam::Const(_)))
}

struct AppendCapture {
    capture: PreciseCapture,
}

impl VisitMut for AppendCapture {
    fn visit_type_impl_trait_mut(&mut self, node: &mut TypeImplTrait) {
        let keeps_captures = node.bounds.iter().any(|bound| match bound {
            TypeParamBound::PreciseCapture(_) => true,
            other => names_lifetime(other),
        });
        visit_mut::visit_type_impl_trait_mut(self, node);
        if !keeps_captures {
            node.bounds.push(TypeParamBound::PreciseCapture(self.capture.clone()));
        }
    }
}

/// Whether `bound` names a lifetime other than `'static`, or holds a reference whose lifetime is
/// elided: either ties the opaque type to a lifetime it must capture. A lifetime bound under a
/// `for<..>`, and an elided one in `Fn(&str)` or `fn(&str)` sugar, is late-bound and captures
/// nothing.
fn names_lifetime(bound: &TypeParamBound) -> bool {
    let mut finder = LifetimeFinder { found: false };
    finder.visit_type_param_bound_mut(&mut bound.clone());
    finder.found
}

struct LifetimeFinder {
    found: bool,
}

impl VisitMut for LifetimeFinder {
    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        if lifetime.ident != "static" {
            self.found = true;
        }
    }

    fn visit_type_reference_mut(&mut self, reference: &mut TypeReference) {
        if reference.lifetime.is_none() {
            self.found = true;
        }
        visit_mut::visit_type_reference_mut(self, reference);
    }

    fn visit_trait_bound_mut(&mut self, bound: &mut TraitBound) {
        if bound.lifetimes.is_none() {
            visit_mut::visit_trait_bound_mut(self, bound);
        }
    }

    fn visit_parenthesized_generic_arguments_mut(&mut self, _: &mut ParenthesizedGenericArguments) {}

    fn visit_type_bare_fn_mut(&mut self, _: &mut TypeBareFn) {}
}
