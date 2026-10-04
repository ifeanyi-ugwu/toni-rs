//! `#[message("event")]`: one `Ws` handler over `ulo-handler-codegen`, whose handler value calls
//! `<Self as GatewayConfig>::mount_gateway` through the mount function's `Mount`.

use proc_macro2::TokenStream;

/// The transport's key, which equals `<ulo_ws::Ws as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "ws";

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let _ = (attr, item, KEY);
    todo!()
}
