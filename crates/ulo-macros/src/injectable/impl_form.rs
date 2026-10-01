//! The impl form: the constructor is the fn named `new`, or the one marked `#[construct]`. Two
//! marked fns, or neither a marked fn nor `new`, is a span error. Its parameters are sites; it
//! may be `async`; its return is `Self` or `Result<Self, E>`, told apart by type through
//! `::ulo::__private::IntoConstructed` rather than by the spelling of the return type, so an
//! alias for a `Result` reads as one. A constructor whose future is not `Send` fails at the
//! generated `Construct` impl, pointing at the function.

use proc_macro2::TokenStream;
use syn::{ImplItemFn, ItemImpl};

use crate::injectable::InjectableArgs;
use crate::shared::sites::SiteSpec;

/// The impl as written, minus `#[construct]`, followed by the `Construct` impl for its self type.
pub(crate) fn expand(args: InjectableArgs, mut item: ItemImpl) -> syn::Result<TokenStream> {
    todo!()
}

/// The constructor: the one fn marked `#[construct]` (stripped), else the fn named `new`.
pub(crate) fn find_constructor(item: &mut ItemImpl) -> syn::Result<&ImplItemFn> {
    todo!()
}

/// The constructor's parameters as sites. A receiver, or a pattern other than an identifier,
/// is a span error on it.
pub(crate) fn params(ctor: &ImplItemFn) -> syn::Result<Vec<SiteSpec>> {
    todo!()
}
