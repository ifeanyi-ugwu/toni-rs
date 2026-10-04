//! `#[message("pattern")]`: one `Rpc` request-reply handler over `ulo-handler-codegen`.

use proc_macro2::TokenStream;

/// The transport's key, which equals `<ulo_rpc::Rpc as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "rpc";

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let _ = (attr, item, KEY);
    todo!()
}
