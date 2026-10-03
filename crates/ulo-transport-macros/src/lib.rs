//! `#[derive(Classify)]` and `#[derive(Validate)]` for `ulo-transport` (transports DESIGN §2.2,
//! §2.4). The generated impls name `::ulo_transport`, so a crate deriving either depends on
//! `ulo-transport`.

mod classify;
mod validate;

use proc_macro::TokenStream;

/// `impl ::ulo_transport::Classify` from `#[classify(<kind>)]`, the kind a snake-case
/// `ErrorKind` variant: `bad_request`, `unauthorized`, `forbidden`, `not_found`, `conflict`,
/// `unprocessable`, `too_many_requests`, `timeout`, `unavailable`, `unimplemented`, `internal`.
///
/// On a struct, one `#[classify(..)]` on the type. On an enum, one on each variant, or one on the
/// enum as the default for variants that carry none; a variant left with no kind is a span error
/// on it. The helper attribute is scoped to the derive, so it sits beside thiserror's `#[error]`:
///
/// ```ignore
/// #[derive(Debug, thiserror::Error, Classify)]
/// pub enum UserError {
///     #[error("user not found")] #[classify(not_found)] NotFound,
///     #[error("email already registered")] #[classify(conflict)] Taken,
/// }
/// ```
#[proc_macro_derive(Classify, attributes(classify))]
pub fn derive_classify(input: TokenStream) -> TokenStream {
    classify::expand(input.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// `impl ::ulo_transport::Validate` from `#[validate(..)]` on the fields of a struct with named
/// fields: `length(min = .., max = ..)` on text, counted in characters, or on a collection,
/// counted in items; `email` on a string; `range(min = .., max = ..)` on an ordered value. Bounds
/// are inclusive, either may be left out, and a bound's value is written into the violation's
/// description, so it implements `Display`. Several rules go in one attribute or in several:
/// `#[validate(length(max = 254), email)]`.
///
/// Every failing rule is one `FieldViolation`, so a value is checked whole rather than to its
/// first failure. The violation names the field as the request does: serde's `rename` or the
/// container's `rename_all` when present. A field with no `#[validate]` is not checked.
#[proc_macro_derive(Validate, attributes(validate))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    validate::expand(input.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
