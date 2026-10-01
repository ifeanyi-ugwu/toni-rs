//! Sites as the macros see them: a field or parameter with its type, emitted three ways: its
//! declaration in `Construct::sites`, its read in `Construct::construct`, and a `quote_spanned!`
//! assertion that points a site error at the site itself.

use proc_macro2::{Span, TokenStream};
use syn::{Ident, Type};

pub(crate) struct SiteSpec {
    pub(crate) label: SiteLabel,
    pub(crate) ty: Type,
    /// The span of the site's type, where its assertion's error points.
    pub(crate) span: Span,
}

pub(crate) enum SiteLabel {
    Field(Ident),
    /// A tuple struct's field, declared positionally with `s.site::<Ty>()`.
    Index(usize),
    Param(Ident),
}

/// `s.field::<Ty>("name");` or `s.param::<Ty>("name");` for each site, each followed by
/// `::ulo::__private::assert_site::<Ty, #scope>()` spanned at the site's type.
pub(crate) fn declare(sites: &[SiteSpec], scope: &TokenStream) -> TokenStream {
    todo!()
}

/// `let #binding = <#ty as ::ulo::Site>::read(r).await?;` for each site, in order. A failed read
/// propagates as `ConstructError::Site` through `From<LookupError>`.
pub(crate) fn read(sites: &[SiteSpec], bindings: &[Ident]) -> TokenStream {
    todo!()
}
