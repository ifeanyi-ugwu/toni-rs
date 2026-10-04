//! `#[gateway(..)]`: the impl's `GatewayConfig`, its `mount_gateway` probing the connection hooks
//! at the concrete type.

use proc_macro2::TokenStream;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let _ = (attr, item);
    todo!()
}
