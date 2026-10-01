//! `#[module(..)]`: a `Module` impl whose `register` calls the value API (§4, §8.1).
//!
//! A unit struct is identified by type (`ModuleIdentity::of_type::<Self>()`); a struct with
//! fields is a configured module, identified by value (`ModuleIdentity::of_value(self)`, which
//! needs `Eq + Hash + Clone`), and every `Secret<_>` field is registered with `m.secret(..)`.
//! `register` never returns early: `expr?` lowers to `m.try_value(expr)`.

mod args;
mod providers;

use proc_macro2::TokenStream;
use syn::ItemStruct;

pub(crate) use args::{ExportEntry, ModuleArgs};
pub(crate) use providers::ProviderEntry;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: ModuleArgs = syn::parse2(attr)?;
    let item: ItemStruct = syn::parse2(item)?;
    expand_struct(args, item)
}

/// The struct as written, followed by its `Module` impl.
pub(crate) fn expand_struct(args: ModuleArgs, item: ItemStruct) -> syn::Result<TokenStream> {
    todo!()
}

/// `m.secret(&self.<field>);` for each field whose type's last path segment is `Secret`.
pub(crate) fn secret_fields(item: &ItemStruct) -> TokenStream {
    todo!()
}
