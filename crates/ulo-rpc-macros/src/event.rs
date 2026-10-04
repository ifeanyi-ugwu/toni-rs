//! `#[event("pattern")]`: one `Rpc` fire-and-forget handler over `ulo-handler-codegen`.

use proc_macro2::TokenStream;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let _ = (attr, item, crate::message::KEY);
    todo!()
}
