use proc_macro2::TokenStream;
use syn::{DeriveInput, Expr, Ident};

/// One rule on one field.
pub(crate) enum Rule {
    /// `length(min = .., max = ..)`, either bound optional, checked against `.len()`.
    Length { min: Option<Expr>, max: Option<Expr> },
    /// `email`: one `@` with a non-empty local part and a domain holding a `.`; not the full
    /// RFC 5322 grammar.
    Email,
    /// `range(min = .., max = ..)`, either bound optional, inclusive.
    Range { min: Option<Expr>, max: Option<Expr> },
}

/// The rules of one field.
pub(crate) struct FieldRules {
    pub(crate) field: Ident,
    pub(crate) rules: Vec<Rule>,
}

/// `impl ::ulo_transport::Validate for <Type>`, collecting a `FieldViolation` per failing rule.
pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let _ = input;
    todo!("refuse anything but a struct with named fields; read each field's `#[validate(..)]`; write `validate`")
}

/// The rules in a field's `#[validate(..)]` attributes; an unknown rule is a span error naming the
/// three.
pub(crate) fn rules_of(attrs: &[syn::Attribute]) -> syn::Result<Vec<Rule>> {
    let _ = attrs;
    todo!("parse `length(..)`, `email` and `range(..)`")
}
