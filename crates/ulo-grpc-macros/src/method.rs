//! `#[method(Marker)]`: one `Grpc` handler over `ulo-handler-codegen`, its shape read from the
//! marker and checked against the signature.

use proc_macro2::TokenStream;

/// The transport's key, which equals `<ulo_grpc::Grpc as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "grpc";

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let _ = (attr, item, KEY);
    todo!()
}
