//! `#[construct]` outside an `#[injectable]` impl: `#[injectable]` strips the marker before the
//! compiler sees it, so reaching this expansion means it was written somewhere it means nothing.

use proc_macro2::{Span, TokenStream};

pub(crate) fn expand(_attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(syn::Error::new(
        Span::call_site(),
        "#[construct] marks the constructor inside an #[injectable] impl block",
    ))
}
