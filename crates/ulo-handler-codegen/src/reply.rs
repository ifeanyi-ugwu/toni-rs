//! The reply side of a handler: the probe that sends every error side to the error handlers
//! whatever the return type is spelled as, and the `+ use<>` rewrite on streaming returns
//! (transports DESIGN §2.3).

use proc_macro2::TokenStream;
use syn::{Ident, Signature};

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
    let _ = (out, cx, paths);
    todo!("the autoref call above, spanned at the call site, `cx` passed by reference")
}

/// Appends `+ use<>` to every opaque type in `sig`'s return position, inside an `async fn`'s
/// return type included, so a reply outlives the call: on edition 2024 an opaque return type
/// captures `&self`'s lifetime whatever the hidden type borrows.
///
/// Left as written: every opaque type of a handler with a type or const parameter, and an opaque
/// type that already names a `use<..>` bound or a lifetime. A stream that borrows `self` then fails
/// inside the handler body; the idiom is `self: Arc<Self>` as the receiver.
pub fn rewrite_opaque_returns(sig: &mut Signature) {
    let _ = sig;
    todo!("walk `sig.output` with `syn::visit_mut`, push `TypeParamBound::PreciseCapture(use<>)` onto each `Type::ImplTrait` without a lifetime or `use<..>` bound; no-op when `sig.generics` has a type or const parameter")
}
