//! `#[event("pattern")]`: one `Rpc` fire-and-forget handler over `ulo-handler-codegen`, expanded as
//! `#[message]` is with `Kind::Event`. A stream return is refused here, an `Inbound<T>` parameter
//! in `prepare`.

use proc_macro2::TokenStream;

use crate::message::{Kind, expand_kind};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    expand_kind(Kind::Event, attr, item)
}
