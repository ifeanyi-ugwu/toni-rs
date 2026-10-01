//! The struct form: every field is a site, read in declaration order, except a field marked
//! `#[injectable(default)]`, set from `Default::default()`. Tuple and unit structs are accepted;
//! a tuple field's site is labelled by its index.

use proc_macro2::TokenStream;
use syn::{Field, ItemStruct};

use crate::injectable::InjectableArgs;
use crate::shared::sites::SiteSpec;

/// The struct as written, minus its `#[injectable(default)]` field markers, followed by its
/// `Construct` impl.
pub(crate) fn expand(args: InjectableArgs, mut item: ItemStruct) -> syn::Result<TokenStream> {
    todo!()
}

/// What one field contributes: a site, or a default.
pub(crate) enum FieldRole {
    Site(SiteSpec),
    Default,
}

/// Reads and strips the field's `#[injectable(default)]`; any other argument there is a span
/// error on it.
pub(crate) fn classify(field: &mut Field, index: usize) -> syn::Result<FieldRole> {
    todo!()
}
