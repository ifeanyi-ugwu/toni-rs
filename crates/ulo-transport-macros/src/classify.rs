use proc_macro2::{Span, TokenStream};
use syn::{DeriveInput, Ident};

/// The kind words `#[classify(..)]` takes, and the `ErrorKind` variant each names.
pub(crate) const KINDS: &[(&str, &str)] = &[
    ("bad_request", "BadRequest"),
    ("unauthorized", "Unauthorized"),
    ("forbidden", "Forbidden"),
    ("not_found", "NotFound"),
    ("conflict", "Conflict"),
    ("unprocessable", "Unprocessable"),
    ("too_many_requests", "TooManyRequests"),
    ("timeout", "Timeout"),
    ("unavailable", "Unavailable"),
    ("unimplemented", "Unimplemented"),
    ("internal", "Internal"),
];

/// One `#[classify(<kind>)]`: the `ErrorKind` variant it names, with the word's span.
pub(crate) struct KindArg {
    pub(crate) variant: Ident,
    pub(crate) span: Span,
}

/// `impl ::ulo_transport::Classify for <Type>`, its `classify` matching each variant to its kind.
pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let _ = input;
    todo!("read the type-level and per-variant `#[classify(..)]`, refuse a variant with no kind, and write `classify`")
}

/// The `#[classify(..)]` among `attrs`, if any; a word outside [`KINDS`] or a second attribute is
/// a span error.
pub(crate) fn kind_of(attrs: &[syn::Attribute]) -> syn::Result<Option<KindArg>> {
    let _ = attrs;
    todo!("parse one identifier and map it through `KINDS`")
}
