//! Collecting `#[set_metadata(...)]` from an impl block and its handlers.
//!
//! Every structural macro reads the attribute at both levels and emits the impl block's entries
//! before the handler's. A later `insert` on the same type shadows an earlier one, so the handler
//! wins where both annotate one type — the result Nest reaches by searching
//! `[getHandler(), getClass()]` in order, settled here at expansion instead of at every read.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Result};

use crate::shared::attr_is;

/// The `#[set_metadata(...)]` expressions on one attribute list — an impl block's or a handler's.
pub fn get_metadata_exprs(attrs: &[Attribute]) -> Result<Vec<TokenStream>> {
    let mut exprs = Vec::new();
    for attr in attrs {
        if attr_is(attr, "set_metadata") {
            let expr: syn::Expr = attr.parse_args()?;
            exprs.push(quote! { #expr });
        }
    }
    Ok(exprs)
}

/// The impl block's entries followed by the handler's, in the order they must be inserted.
pub fn merged_metadata_exprs(
    impl_attrs: &[Attribute],
    handler_attrs: &[Attribute],
) -> Result<Vec<TokenStream>> {
    let mut exprs = get_metadata_exprs(impl_attrs)?;
    exprs.extend(get_metadata_exprs(handler_attrs)?);
    Ok(exprs)
}

/// Build a `Metadata` from collected expressions, or `None` when there are none to insert.
pub fn metadata_ctor(exprs: &[TokenStream]) -> Option<TokenStream> {
    if exprs.is_empty() {
        return None;
    }
    Some(quote! {
        {
            let mut __metadata = ::toni::context::Metadata::new();
            #(__metadata.insert(#exprs);)*
            __metadata
        }
    })
}
