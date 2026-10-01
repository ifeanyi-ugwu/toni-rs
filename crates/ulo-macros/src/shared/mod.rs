//! Helpers every macro uses: the path to the core, attribute handling, and site emission.

pub(crate) mod attrs;
pub(crate) mod sites;

use proc_macro2::TokenStream;
use quote::quote;

/// The path generated code names the core by. The macros are used through `ulo`'s re-exports,
/// so `::ulo` resolves wherever they expand.
pub(crate) fn ulo() -> TokenStream {
    quote!(::ulo)
}

/// The `Construct` impl both forms of `#[injectable]` write: scope, optional `CONSTRUCT_TIMEOUT`,
/// `sites`, `construct` and the probing `hooks`.
pub(crate) struct ConstructImpl<'a> {
    pub(crate) self_ty: &'a syn::Type,
    pub(crate) generics: &'a syn::Generics,
    /// `::ulo::scope::Auto` when no scope is written.
    pub(crate) scope: TokenStream,
    pub(crate) timeout: Option<&'a syn::Expr>,
    /// The body of `fn sites(s: &mut ::ulo::Sites)`, assertions included.
    pub(crate) sites: TokenStream,
    /// The body of `async fn construct(r: &::ulo::Resolver<'_>) -> Result<Self, ::ulo::ConstructError>`.
    pub(crate) construct: TokenStream,
}

impl ConstructImpl<'_> {
    pub(crate) fn emit(&self) -> TokenStream {
        todo!()
    }
}
